//! How good recognition is, measured on the secretary's own work.
//!
//! A meeting the secretary has checked holds everything a measurement or a
//! training run needs: the recording and the corrected lines with their
//! times. Nothing has to be collected separately, and meetings checked
//! before this existed count too.
//!
//! - [`pairs`]: checked lines of a meeting cut into pieces of the recording
//!   with their right text (up to 25 s, the size Whisper works with);
//! - [`score`]: what recognition got wrong in a piece and how many of those
//!   mistakes the underlined («сумнівні») words point to;
//! - [`wav`]: the pieces as 16 kHz mono WAV files.
//!
//! Everything stays on the Mac it runs on. The measurement itself runs from
//! `eval_pipeline` (stages `export` and `replay`).

pub mod pairs;
pub mod score;
pub mod wav;
