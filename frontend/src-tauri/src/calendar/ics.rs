//! Minimal iCalendar (RFC 5545) reader for meeting reminders.
//!
//! Reads VEVENTs from a calendar feed (Google Calendar "secret address in
//! iCal format", Outlook / Exchange published calendars, or an exported .ics
//! file), expands recurring meetings and returns the meetings that start in a
//! given time window. All-day events and cancelled meetings are skipped.

use std::collections::HashSet;
use std::str::FromStr;

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};
use rrule::{RRule, Tz, Unvalidated};
use serde::Serialize;

/// One meeting occurrence.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CalendarEvent {
    pub uid: String,
    pub title: String,
    pub start: DateTime<Utc>,
    pub end: Option<DateTime<Utc>>,
    /// Link to the online call (Meet, Zoom, Teams, Webex), if one is found.
    pub meeting_url: Option<String>,
    pub location: Option<String>,
}

impl CalendarEvent {
    /// Stable key for one occurrence, used to remind only once.
    pub fn occurrence_key(&self) -> String {
        format!("{}|{}", self.uid, self.start.timestamp())
    }
}

/// Upper bound on occurrences expanded per recurring meeting.
const MAX_OCCURRENCES_PER_SERIES: u16 = 500;

struct Property {
    name: String,
    params: Vec<(String, String)>,
    value: String,
}

impl Property {
    fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// Returns the meetings that start within `[from, to]`, sorted by start time.
pub fn events_between(ics: &str, from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<CalendarEvent> {
    let mut singles: Vec<CalendarEvent> = Vec::new();
    let mut expanded: Vec<CalendarEvent> = Vec::new();
    // (uid, start) of occurrences that a RECURRENCE-ID override replaces.
    let mut overridden: HashSet<(String, i64)> = HashSet::new();

    for props in vevents(ics) {
        let get = |name: &str| props.iter().find(|p| p.name.eq_ignore_ascii_case(name));
        let Some(dtstart) = get("DTSTART") else { continue };
        let Some(start) = parse_datetime(&dtstart.value, dtstart.param("TZID"), dtstart.param("VALUE")) else {
            continue; // all-day or unreadable
        };
        let uid = get("UID").map(|p| p.value.clone()).unwrap_or_else(|| format!("no-uid-{}", start.timestamp()));
        let cancelled = get("STATUS").is_some_and(|p| p.value.eq_ignore_ascii_case("CANCELLED"));

        if let Some(recurrence_id) = get("RECURRENCE-ID") {
            if let Some(original) =
                parse_datetime(&recurrence_id.value, recurrence_id.param("TZID"), recurrence_id.param("VALUE"))
            {
                overridden.insert((uid.clone(), original.with_timezone(&Utc).timestamp()));
            }
        }
        if cancelled {
            continue;
        }

        let duration = get("DTEND")
            .and_then(|p| parse_datetime(&p.value, p.param("TZID"), p.param("VALUE")))
            .map(|end| end.with_timezone(&Utc) - start.with_timezone(&Utc))
            .or_else(|| get("DURATION").and_then(|p| parse_duration(&p.value)))
            .filter(|d| *d > Duration::zero());

        let title = get("SUMMARY")
            .map(|p| unescape_text(&p.value))
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| "Нарада без назви".to_string());
        let location = get("LOCATION").map(|p| unescape_text(&p.value)).filter(|l| !l.trim().is_empty());
        let meeting_url = ["X-GOOGLE-CONFERENCE", "URL", "LOCATION", "DESCRIPTION"]
            .iter()
            .filter_map(|name| get(name))
            .find_map(|p| find_meeting_url(&unescape_text(&p.value)));

        let make = |start: DateTime<Utc>| CalendarEvent {
            uid: uid.clone(),
            title: title.clone(),
            start,
            end: duration.map(|d| start + d),
            meeting_url: meeting_url.clone(),
            location: location.clone(),
        };

        let rrule_prop = get("RRULE").filter(|_| get("RECURRENCE-ID").is_none());
        match rrule_prop.and_then(|p| expand_rrule(&p.value, start, &props, from, to)) {
            Some(starts) => expanded.extend(starts.into_iter().map(make)),
            None => {
                let start = start.with_timezone(&Utc);
                if start >= from && start <= to {
                    singles.push(make(start));
                }
            }
        }
    }

    expanded.retain(|e| !overridden.contains(&(e.uid.clone(), e.start.timestamp())));
    let mut all = singles;
    all.extend(expanded);
    all.sort_by_key(|e| e.start);
    all.dedup_by(|a, b| a.uid == b.uid && a.start == b.start);
    all
}

fn expand_rrule(
    rule: &str,
    start: DateTime<Tz>,
    props: &[Property],
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Option<Vec<DateTime<Utc>>> {
    let parsed: RRule<Unvalidated> = RRule::from_str(rule).ok()?;
    let mut set = parsed.build(start).ok()?;
    for exdate in props.iter().filter(|p| p.name.eq_ignore_ascii_case("EXDATE")) {
        for value in exdate.value.split(',') {
            if let Some(dt) = parse_datetime(value.trim(), exdate.param("TZID"), exdate.param("VALUE")) {
                set = set.exdate(dt);
            }
        }
    }
    let utc = Tz::Tz(chrono_tz::UTC);
    let result = set
        .after(utc.from_utc_datetime(&(from - Duration::seconds(1)).naive_utc()))
        .before(utc.from_utc_datetime(&(to + Duration::seconds(1)).naive_utc()))
        .all(MAX_OCCURRENCES_PER_SERIES);
    Some(
        result
            .dates
            .into_iter()
            .map(|d| d.with_timezone(&Utc))
            .filter(|d| *d >= from && *d <= to)
            .collect(),
    )
}

/// Parses DTSTART-style values. Returns None for all-day dates.
fn parse_datetime(value: &str, tzid: Option<&str>, value_type: Option<&str>) -> Option<DateTime<Tz>> {
    let value = value.trim();
    if value_type.is_some_and(|v| v.eq_ignore_ascii_case("DATE")) || value.len() == 8 {
        let _ = NaiveDate::parse_from_str(value, "%Y%m%d").ok()?;
        return None;
    }
    if let Some(utc_value) = value.strip_suffix('Z') {
        let naive = NaiveDateTime::parse_from_str(utc_value, "%Y%m%dT%H%M%S").ok()?;
        return Some(Tz::Tz(chrono_tz::UTC).from_utc_datetime(&naive));
    }
    let naive = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S").ok()?;
    let tz = tzid.map(resolve_timezone).unwrap_or(Tz::LOCAL);
    tz.from_local_datetime(&naive).earliest()
}

/// IANA names (Google) and common Windows names (Outlook / Exchange).
fn resolve_timezone(tzid: &str) -> Tz {
    let tzid = tzid.trim().trim_matches('"');
    if let Ok(tz) = chrono_tz::Tz::from_str(tzid) {
        return Tz::Tz(tz);
    }
    let iana = match tzid {
        "FLE Standard Time" => "Europe/Kyiv",
        "E. Europe Standard Time" => "Europe/Chisinau",
        "GTB Standard Time" => "Europe/Bucharest",
        "Central European Standard Time" => "Europe/Warsaw",
        "Central Europe Standard Time" => "Europe/Budapest",
        "W. Europe Standard Time" => "Europe/Berlin",
        "Romance Standard Time" => "Europe/Paris",
        "GMT Standard Time" => "Europe/London",
        "Turkey Standard Time" => "Europe/Istanbul",
        "Eastern Standard Time" => "America/New_York",
        "Pacific Standard Time" => "America/Los_Angeles",
        "UTC" | "Coordinated Universal Time" => "UTC",
        _ => return Tz::LOCAL,
    };
    chrono_tz::Tz::from_str(iana).map(Tz::Tz).unwrap_or(Tz::LOCAL)
}

/// Parses simple ISO 8601 durations such as PT1H30M or P1DT2H.
fn parse_duration(value: &str) -> Option<Duration> {
    let value = value.trim().trim_start_matches('+');
    let rest = value.strip_prefix('P')?;
    let mut total = Duration::zero();
    let mut number = String::new();
    let mut in_time = false;
    for c in rest.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' => number.push(c),
            unit => {
                let n: i64 = number.parse().ok()?;
                number.clear();
                total += match (unit, in_time) {
                    ('W', _) => Duration::weeks(n),
                    ('D', _) => Duration::days(n),
                    ('H', true) => Duration::hours(n),
                    ('M', true) => Duration::minutes(n),
                    ('S', true) => Duration::seconds(n),
                    _ => return None,
                };
            }
        }
    }
    Some(total)
}

const MEETING_HOSTS: &[&str] = &[
    "meet.google.com",
    "zoom.us",
    "teams.microsoft.com",
    "teams.live.com",
    "webex.com",
];

/// Finds the first online-meeting link in free text.
pub fn find_meeting_url(text: &str) -> Option<String> {
    let mut rest = text;
    while let Some(pos) = rest.find("https://") {
        let candidate: String = rest[pos..]
            .chars()
            .take_while(|c| !c.is_whitespace() && !matches!(c, '<' | '>' | '"' | '\'' | ')' | ']'))
            .collect();
        let candidate = candidate.trim_end_matches(['.', ',', ';']).to_string();
        let host = candidate["https://".len()..].split(['/', '?', '#']).next().unwrap_or("");
        if MEETING_HOSTS.iter().any(|h| host == *h || host.ends_with(&format!(".{h}"))) {
            return Some(candidate);
        }
        rest = &rest[pos + "https://".len()..];
    }
    None
}

/// Splits the calendar into VEVENT property lists (alarms are ignored).
fn vevents(ics: &str) -> Vec<Vec<Property>> {
    let mut events = Vec::new();
    let mut current: Option<Vec<Property>> = None;
    let mut nested_depth = 0usize;
    for line in unfold(ics) {
        let Some(prop) = parse_property(&line) else { continue };
        match (prop.name.to_ascii_uppercase().as_str(), prop.value.to_ascii_uppercase().as_str()) {
            ("BEGIN", "VEVENT") => {
                current = Some(Vec::new());
                nested_depth = 0;
            }
            ("END", "VEVENT") => {
                if let Some(props) = current.take() {
                    events.push(props);
                }
            }
            ("BEGIN", _) if current.is_some() => nested_depth += 1,
            ("END", _) if current.is_some() => nested_depth = nested_depth.saturating_sub(1),
            _ => {
                if nested_depth == 0 {
                    if let Some(props) = current.as_mut() {
                        props.push(prop);
                    }
                }
            }
        }
    }
    events
}

/// Joins folded lines (a line break followed by a space or tab).
fn unfold(ics: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in ics.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(continuation) = raw.strip_prefix([' ', '\t']) {
            if let Some(last) = lines.last_mut() {
                last.push_str(continuation);
                continue;
            }
        }
        if !raw.is_empty() {
            lines.push(raw.to_string());
        }
    }
    lines
}

fn parse_property(line: &str) -> Option<Property> {
    let mut in_quotes = false;
    let mut colon = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            ':' if !in_quotes => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let mut parts = split_unquoted(head, ';').into_iter();
    let name = parts.next()?.trim().to_string();
    let params = parts
        .filter_map(|p| {
            let (k, v) = p.split_once('=')?;
            Some((k.trim().to_string(), v.trim().trim_matches('"').to_string()))
        })
        .collect();
    Some(Property { name, params, value: value.to_string() })
}

fn split_unquoted(s: &str, sep: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut in_quotes = false;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        if c == '"' {
            in_quotes = !in_quotes;
        } else if c == sep && !in_quotes {
            parts.push(&s[start..i]);
            start = i + c.len_utf8();
        }
    }
    parts.push(&s[start..]);
    parts
}

fn unescape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    const FEED: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:single@x\r\nDTSTART:20261104T090000Z\r\nDTEND:20261104T100000Z\r\n\
SUMMARY:Нарада офісу\\, щотижнева\r\nDESCRIPTION:Приєднатися: https://meet.google.com/abc-defg-hij\\n\r\n\
BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Reminder\r\nEND:VALARM\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:weekly@x\r\nDTSTART;TZID=Europe/Kiev:20261102T100000\r\nDTEND;TZID=Europe/Kiev:20261102T103000\r\n\
RRULE:FREQ=WEEKLY;BYDAY=MO,WE\r\nEXDATE;TZID=Europe/Kiev:20261104T100000\r\nSUMMARY:Планерка\r\n\
LOCATION:https://us02web.zoom.us/j/123?pwd=x\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:weekly@x\r\nRECURRENCE-ID;TZID=Europe/Kiev:20261109T100000\r\n\
DTSTART;TZID=Europe/Kiev:20261109T120000\r\nDTEND;TZID=Europe/Kiev:20261109T123000\r\nSUMMARY:Планерка (перенесено)\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:allday@x\r\nDTSTART;VALUE=DATE:20261105\r\nSUMMARY:Відпустка\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:cancel@x\r\nDTSTART:20261105T090000Z\r\nSTATUS:CANCELLED\r\nSUMMARY:Скасовано\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:folded@x\r\nDTSTART:20261105T130000Z\r\nDURATION:PT1H30M\r\nSUMMARY:Довга на\r\n зва\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";

    #[test]
    fn reads_single_meeting_with_link() {
        let events = events_between(FEED, utc("2026-11-04T00:00:00Z"), utc("2026-11-04T23:59:59Z"));
        let titles: Vec<_> = events.iter().map(|e| e.title.as_str()).collect();
        // Wednesday 4 Nov: the single meeting; the weekly one is excluded by EXDATE.
        assert_eq!(titles, vec!["Нарада офісу, щотижнева"]);
        assert_eq!(events[0].meeting_url.as_deref(), Some("https://meet.google.com/abc-defg-hij"));
        assert_eq!(events[0].end, Some(utc("2026-11-04T10:00:00Z")));
    }

    #[test]
    fn expands_recurring_meetings_in_their_time_zone() {
        let events = events_between(FEED, utc("2026-11-02T00:00:00Z"), utc("2026-11-03T00:00:00Z"));
        assert_eq!(events.len(), 1);
        // 10:00 Kyiv in November is 08:00 UTC.
        assert_eq!(events[0].start, utc("2026-11-02T08:00:00Z"));
        assert_eq!(events[0].meeting_url.as_deref(), Some("https://us02web.zoom.us/j/123?pwd=x"));
        assert_eq!(events[0].end, Some(utc("2026-11-02T08:30:00Z")));
    }

    #[test]
    fn moved_occurrence_replaces_the_original() {
        let events = events_between(FEED, utc("2026-11-09T00:00:00Z"), utc("2026-11-09T23:59:59Z"));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].title, "Планерка (перенесено)");
        assert_eq!(events[0].start, utc("2026-11-09T10:00:00Z"));
    }

    #[test]
    fn skips_all_day_and_cancelled_and_unfolds_lines() {
        let events = events_between(FEED, utc("2026-11-05T00:00:00Z"), utc("2026-11-05T23:59:59Z"));
        let titles: Vec<_> = events.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, vec!["Довга назва"]);
        assert_eq!(events[0].end, Some(utc("2026-11-05T14:30:00Z")));
    }

    #[test]
    fn windows_time_zone_names_are_understood() {
        let ics = "BEGIN:VEVENT\nUID:o@x\nDTSTART;TZID=\"FLE Standard Time\":20260701T100000\nSUMMARY:Outlook\nEND:VEVENT\n";
        let events = events_between(ics, utc("2026-07-01T00:00:00Z"), utc("2026-07-02T00:00:00Z"));
        // Kyiv summer time is UTC+3.
        assert_eq!(events[0].start, utc("2026-07-01T07:00:00Z"));
    }

    #[test]
    fn meeting_links_are_recognised() {
        assert_eq!(
            find_meeting_url("Join: https://teams.microsoft.com/l/meetup-join/19%3a>"),
            Some("https://teams.microsoft.com/l/meetup-join/19%3a".to_string())
        );
        assert_eq!(find_meeting_url("Docs https://docs.google.com/x only"), None);
        assert_eq!(find_meeting_url("https://evil-zoom.us.example.com/j/1"), None);
    }

    #[test]
    fn durations_are_parsed() {
        assert_eq!(parse_duration("PT1H30M"), Some(Duration::minutes(90)));
        assert_eq!(parse_duration("P1DT2H"), Some(Duration::hours(26)));
        assert_eq!(parse_duration("garbage"), None);
    }
}
