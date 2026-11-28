//! Listed words made a little more likely while Whisper decodes.
//!
//! Measurement only: nothing in the app turns it on; a piece is recognised
//! with a list only inside [`BOOST`]`.scope(…)`.
//!
//! Whisper picks every next token from its own scores. Here the tokens that
//! begin a listed word get `start` added to their score, and the tokens that
//! continue a listed word already begun get `cont`. The sound still decides:
//! a word that was not said does not appear because it is listed, but of two
//! near spellings the listed one wins.

use std::collections::HashMap;
use std::os::raw::{c_int, c_void};
use std::sync::{Arc, OnceLock};

use whisper_rs::whisper_rs_sys::{whisper_context, whisper_state, whisper_token_data};
use whisper_rs::WhisperContext;

tokio::task_local! {
    /// The words to favour while a piece is recognised inside
    /// [`BOOST`]`.scope(…)`
    pub static BOOST: Arc<Boost>;
}

/// The list in use for the piece being recognised, if any
pub fn current() -> Option<Arc<Boost>> {
    BOOST.try_with(|b| b.clone()).ok()
}

/// No listed word is longer than this in tokens (longer ones are left out)
const MAX_TOKENS: usize = 32;

/// Words to favour and by how much
pub struct Boost {
    words: Vec<String>,
    start: f32,
    cont: f32,
    trie: OnceLock<Trie>,
}

impl Boost {
    pub fn new(words: Vec<String>, start: f32, cont: f32) -> Self {
        Self { words, start, cont, trie: OnceLock::new() }
    }

    pub fn len(&self) -> usize {
        self.words.len()
    }

    /// The listed words in the loaded model's own tokens (made once: one
    /// list serves one model)
    pub fn trie(&self, ctx: &WhisperContext) -> &Trie {
        self.trie.get_or_init(|| {
            let mut trie = Trie::new(self.start, self.cont, ctx.n_vocab().max(0) as usize);
            for word in &self.words {
                for form in forms(word) {
                    // Inside a line every word follows a space, and Whisper
                    // writes the first word of a piece with one too
                    if let Ok(tokens) = ctx.tokenize(&format!(" {form}"), MAX_TOKENS + 1) {
                        trie.add(&tokens);
                    }
                }
            }
            log::info!("Boost: {} words, {} token paths, start {} cont {}", self.words.len(), trie.paths, self.start, self.cont);
            trie
        })
    }
}

/// A word as it stands and, when it starts with a small letter, as it opens
/// a sentence
fn forms(word: &str) -> Vec<String> {
    let word = word.trim();
    let mut out = Vec::new();
    let Some(first) = word.chars().next() else { return out };
    out.push(word.to_string());
    if first.is_lowercase() {
        let capital: String = first.to_uppercase().chain(word.chars().skip(1)).collect();
        if capital != word {
            out.push(capital);
        }
    }
    out
}

#[derive(Default)]
struct Node {
    next: HashMap<i32, usize>,
    /// A listed word ends here
    end: bool,
}

/// The listed words as paths of tokens from one root
pub struct Trie {
    nodes: Vec<Node>,
    /// The tokens that begin a listed word
    first: Vec<i32>,
    depth: usize,
    paths: usize,
    start: f32,
    cont: f32,
    n_vocab: usize,
}

impl Trie {
    fn new(start: f32, cont: f32, n_vocab: usize) -> Self {
        Self { nodes: vec![Node::default()], first: Vec::new(), depth: 0, paths: 0, start, cont, n_vocab }
    }

    fn add(&mut self, tokens: &[i32]) {
        if tokens.is_empty() || tokens.len() > MAX_TOKENS {
            return;
        }
        let mut node = 0;
        for &t in tokens {
            node = match self.nodes[node].next.get(&t) {
                Some(&n) => n,
                None => {
                    let n = self.nodes.len();
                    self.nodes.push(Node::default());
                    self.nodes[node].next.insert(t, n);
                    if node == 0 {
                        self.first.push(t);
                    }
                    n
                }
            };
        }
        if !self.nodes[node].end {
            self.nodes[node].end = true;
            self.paths += 1;
        }
        self.depth = self.depth.max(tokens.len());
    }

    /// The tokens that would continue a listed word begun at the end of
    /// `tail` (the tokens written so far). A word already whole is not
    /// pushed on into a longer listed one.
    fn continuations(&self, tail: &[i32]) -> Vec<i32> {
        let mut out = Vec::new();
        let tail = &tail[tail.len().saturating_sub(self.depth)..];
        for from in 0..tail.len() {
            let mut node = 0;
            let mut inside = true;
            for t in &tail[from..] {
                match self.nodes[node].next.get(t) {
                    Some(&n) => node = n,
                    None => {
                        inside = false;
                        break;
                    }
                }
            }
            if inside && !self.nodes[node].end {
                out.extend(self.nodes[node].next.keys().copied());
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Adds the bonuses to the scores of the next token: `cont` for a token
    /// that continues a listed word, else `start` for one that begins one
    fn apply(&self, tail: &[i32], logits: &mut [f32]) {
        let continuing = if self.cont != 0.0 { self.continuations(tail) } else { Vec::new() };
        if self.start != 0.0 {
            for &t in &self.first {
                if continuing.binary_search(&t).is_err() {
                    raise(logits, t, self.start);
                }
            }
        }
        for &t in &continuing {
            raise(logits, t, self.cont.max(if self.nodes[0].next.contains_key(&t) { self.start } else { f32::MIN }));
        }
    }
}

fn raise(logits: &mut [f32], token: i32, by: f32) {
    if let Some(l) = usize::try_from(token).ok().and_then(|t| logits.get_mut(t)) {
        // A token Whisper has ruled out (−∞) stays ruled out
        if l.is_finite() {
            *l += by;
        }
    }
}

/// whisper.cpp calls this for every decoder before each next token is
/// picked; `user_data` is the [`Trie`] of the piece's list.
///
/// # Safety
/// `user_data` must point to a `Trie` that lives until `full` returns,
/// `logits` to as many scores as the model has tokens, and `tokens` to
/// `n_tokens` entries (whisper.cpp keeps the last two).
pub unsafe extern "C" fn filter(
    _ctx: *mut whisper_context,
    _state: *mut whisper_state,
    tokens: *const whisper_token_data,
    n_tokens: c_int,
    logits: *mut f32,
    user_data: *mut c_void,
) {
    if user_data.is_null() || logits.is_null() {
        return;
    }
    let trie = &*(user_data as *const Trie);
    let n = if tokens.is_null() { 0 } else { n_tokens.max(0) as usize };
    let mut tail = [0i32; MAX_TOKENS];
    let take = n.min(trie.depth).min(MAX_TOKENS);
    for (k, slot) in tail.iter_mut().enumerate().take(take) {
        *slot = (*tokens.add(n - take + k)).id;
    }
    trie.apply(&tail[..take], std::slice::from_raw_parts_mut(logits, trie.n_vocab));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trie(start: f32, cont: f32) -> Trie {
        let mut t = Trie::new(start, cont, 100);
        t.add(&[10, 11, 12]); // a name
        t.add(&[10, 11, 13]); // the same name in another case
        t.add(&[20]); // a word that is one token
        t.add(&[30, 31]); // a short word …
        t.add(&[30, 31, 32]); // … and a longer one that begins with it
        t
    }

    fn scores(t: &Trie, tail: &[i32]) -> Vec<f32> {
        let mut logits = vec![0.0; t.n_vocab];
        t.apply(tail, &mut logits);
        logits
    }

    #[test]
    fn a_word_begun_is_helped_to_its_end_and_a_new_one_only_a_little() {
        let t = trie(1.0, 4.0);
        // Nothing written yet: only the first tokens of the listed words
        let s = scores(&t, &[]);
        assert_eq!((s[10], s[20], s[30]), (1.0, 1.0, 1.0));
        assert_eq!(s.iter().filter(|v| **v != 0.0).count(), 3);
        // The name begun: its next token, then both of its endings
        let s = scores(&t, &[5, 10]);
        assert_eq!((s[11], s[12], s[10]), (4.0, 0.0, 1.0));
        let s = scores(&t, &[5, 10, 11]);
        assert_eq!((s[12], s[13], s[11]), (4.0, 4.0, 0.0));
        // Written whole: nothing but the beginnings of other words again
        let s = scores(&t, &[5, 10, 11, 12]);
        assert_eq!(s.iter().filter(|v| **v == 4.0).count(), 0);
        assert_eq!(s[10], 1.0);
        // A token of the name elsewhere, not after its beginning: no help
        let s = scores(&t, &[5, 11]);
        assert_eq!(s[12], 0.0);
    }

    #[test]
    fn a_whole_word_is_not_pushed_into_a_longer_listed_one() {
        let t = trie(0.0, 4.0);
        let s = scores(&t, &[30]);
        assert_eq!(s[31], 4.0);
        let s = scores(&t, &[30, 31]);
        assert_eq!(s[32], 0.0);
        // With no bonus for beginnings nothing is touched before a word begins
        assert!(scores(&t, &[]).iter().all(|v| *v == 0.0));
    }

    #[test]
    fn what_whisper_ruled_out_stays_ruled_out() {
        let t = trie(1.0, 4.0);
        let mut logits = vec![0.0; t.n_vocab];
        logits[11] = f32::NEG_INFINITY;
        t.apply(&[10], &mut logits);
        assert_eq!(logits[11], f32::NEG_INFINITY);
        // A token beyond the model's own is ignored
        let mut far = Trie::new(1.0, 4.0, 4);
        far.add(&[2, 9]);
        let mut logits = vec![0.0; 4];
        far.apply(&[2], &mut logits);
        assert_eq!(logits, vec![0.0, 0.0, 1.0, 0.0]);
    }

    #[test]
    fn the_callback_reads_only_the_end_of_what_is_written() {
        let t = trie(1.0, 4.0);
        let token = |id: i32| whisper_token_data { id, tid: 0, p: 0.0, plog: 0.0, pt: 0.0, ptsum: 0.0, t0: 0, t1: 0, t_dtw: 0, vlen: 0.0 };
        let written: Vec<whisper_token_data> = (0..200).map(|_| token(5)).chain([token(10), token(11)]).collect();
        let mut logits = vec![0.0f32; t.n_vocab];
        unsafe {
            filter(std::ptr::null_mut(), std::ptr::null_mut(), written.as_ptr(), written.len() as c_int, logits.as_mut_ptr(), &t as *const Trie as *mut c_void);
        }
        assert_eq!((logits[12], logits[13]), (4.0, 4.0));
        // Nothing written yet (whisper.cpp passes no tokens)
        let mut logits = vec![0.0f32; t.n_vocab];
        unsafe {
            filter(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null(), 0, logits.as_mut_ptr(), &t as *const Trie as *mut c_void);
        }
        assert_eq!(logits[10], 1.0);
    }

    #[test]
    fn a_small_word_is_listed_as_it_opens_a_sentence_too() {
        assert_eq!(forms("кошторис"), vec!["кошторис".to_string(), "Кошторис".to_string()]);
        assert_eq!(forms(" Олена "), vec!["Олена".to_string()]);
        assert!(forms("  ").is_empty());
    }
}
