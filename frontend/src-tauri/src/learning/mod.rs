//! Recognition that learns on this Mac: the model is fine-tuned on the
//! organisation's own checked meetings, at night, and a new model is used
//! only when it is more accurate than the one in use.
//!
//! One run ([`night`]):
//! 1. [`schedule`]: is it time (enough new checked speech, night, power,
//!    nobody using the Mac, enough memory and disk)?
//! 2. [`dataset`]: checked lines cut into pieces of the recordings; the last
//!    checked meetings are held out and never trained on;
//! 3. [`job`], [`runner`]: the trainer (a separate program) gets a job and
//!    reports its progress line by line; it pauses while a recording is on,
//!    and [`watch`] stops it when the Mac struggles;
//! 4. [`measure`], [`gate`]: the new model and the one in use hear the held-out meetings;
//!    the new one is taken only if it makes clearly fewer mistakes and no
//!    meeting gets noticeably worse;
//! 5. [`versions`]: an accepted model becomes active, the previous one is
//!    kept to go back to, every run is written to the history (numbers only).
//!
//! [`commands`] holds what Settings sees and does, and the loop that starts
//! a run at night; [`report`] the numbers a user may choose to send;
//! [`corrected`] how many words of each checked meeting were corrected.
//!
//! Recordings, texts and models never leave the Mac.

pub mod commands;
pub mod corrected;
pub mod dataset;
pub mod engine;
pub mod gate;
pub mod job;
pub mod mac;
pub mod measure;
pub mod night;
pub mod report;
pub mod runner;
pub mod schedule;
pub mod versions;
pub mod watch;
