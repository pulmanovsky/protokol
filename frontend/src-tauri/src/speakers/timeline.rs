//! Combines the local microphone («Я») with the voices found on the call.

use crate::audio::channel_track::ChannelLevels;

use super::Turn;

/// Speaker index used for the person at this Mac's microphone
pub const ME: usize = usize::MAX;
/// Microphone level (RMS × 10 000) that counts as speech
const MIC_SPEECH_LEVEL: u16 = 150;
const MIN_GAP_S: f64 = 0.5;

/// Turns of the local speaker: the microphone carries speech and clearly
/// more energy than the call (headphones keep the call out of the mic).
pub fn me_turns(levels: &ChannelLevels) -> Vec<Turn> {
    let step = levels.step_ms as f64 / 1000.0;
    let n = levels.mic.len().min(levels.system.len());
    let mut turns: Vec<Turn> = Vec::new();
    for i in 0..n {
        let (m, s) = (levels.mic[i] as u64, levels.system[i] as u64);
        let me = levels.mic[i] >= MIC_SPEECH_LEVEL && m * m > 4 * s * s;
        if !me {
            continue;
        }
        let t = i as f64 * step;
        match turns.last_mut() {
            Some(last) if t - last.end <= MIN_GAP_S => last.end = t + step,
            _ => turns.push(Turn { start: t, end: t + step, speaker: ME }),
        }
    }
    super::diarize::smooth_turns(turns)
}

/// Puts the local speaker's turns over the call's turns: where «Я» speaks,
/// the call's turns are cut out.
pub fn overlay(call: &[Turn], me: &[Turn]) -> Vec<Turn> {
    let mut out: Vec<Turn> = Vec::new();
    for t in call {
        let mut pieces = vec![(t.start, t.end)];
        for m in me {
            pieces = pieces
                .into_iter()
                .flat_map(|(a, b)| {
                    if m.end <= a || m.start >= b {
                        vec![(a, b)]
                    } else {
                        let mut v = Vec::new();
                        if m.start > a {
                            v.push((a, m.start));
                        }
                        if m.end < b {
                            v.push((m.end, b));
                        }
                        v
                    }
                })
                .collect();
        }
        out.extend(pieces.into_iter().map(|(a, b)| Turn { start: a, end: b, speaker: t.speaker }));
    }
    out.extend(me.iter().cloned());
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    super::diarize::smooth_turns(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn me_from_the_microphone() {
        let levels = ChannelLevels { step_ms: 100, mic: vec![0, 900, 900, 900, 900, 900, 0, 0, 0, 0], system: vec![500, 10, 10, 10, 10, 10, 500, 500, 500, 500] };
        let me = me_turns(&levels);
        assert_eq!(me.len(), 1);
        assert!((me[0].start - 0.1).abs() < 1e-9 && (me[0].end - 0.6).abs() < 1e-9);
        let call = vec![Turn { start: 0.0, end: 1.0, speaker: 0 }];
        let all = overlay(&call, &me);
        assert_eq!(all.iter().map(|t| t.speaker).collect::<Vec<_>>(), vec![ME, 0]);
    }
}
