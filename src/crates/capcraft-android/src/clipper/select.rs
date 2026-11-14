// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 CapCraft contributors

//! Choosing the moments worth clipping out of a long talk, on the phone.
//!
//! There is no model in this: a transcript says what is said and when, and a
//! window of consecutive lines is scored by what makes a short clip hold
//! someone's attention. A clip should
//!
//! * open on a hook (a question, a bold claim, "did you know"),
//! * be dense with speech, neither rushed nor dragging,
//! * carry some punch (questions, exclamations, numbers),
//! * say something varied rather than filler ("um", "eh", "anu"),
//! * end where a sentence ends, and
//! * be louder than the talk around it, when the sound is known.
//!
//! This file knows nothing of the engine or the bridge: plain seconds and
//! plain strings in, so its tests run anywhere.

/// One line of the transcript, in seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// Where it starts.
    pub start: f64,
    /// Where it ends.
    pub end: f64,
    /// The words.
    pub text: String,
}

/// What kind of clips are wanted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Options {
    /// The shortest clip, seconds.
    pub min_len: f64,
    /// The longest clip, seconds.
    pub max_len: f64,
    /// How many clips at most.
    pub count: usize,
    /// A silence this long ends a scene: a clip never spans one, seconds.
    pub max_gap: f64,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            min_len: 20.0,
            max_len: 60.0,
            count: 5,
            max_gap: 2.5,
        }
    }
}

/// A clip worth making.
#[derive(Debug, Clone, PartialEq)]
pub struct Pick {
    /// Where it starts, seconds, a breath before the first word.
    pub start: f64,
    /// Where it ends, seconds, a breath after the last.
    pub end: f64,
    /// How well it scored, 0 to 1.
    pub score: f64,
    /// The index of its first line.
    pub first: usize,
    /// The index of its last line.
    pub last: usize,
}

/// A breath kept before the first word of a clip.
const LEAD: f64 = 0.20;
/// A breath kept after the last.
const TAIL: f64 = 0.35;
/// The speaking rate a clip should hold, words a second.
const IDEAL_RATE: f64 = 2.8;

/// Phrases that open a clip well, in the languages the app is used in:
/// English and Indonesian.
const HOOKS: &[&str] = &[
    "did you know",
    "have you ever",
    "what if",
    "the secret",
    "the truth",
    "the biggest",
    "the best",
    "the worst",
    "here's why",
    "here is why",
    "never ",
    "stop ",
    "you won't believe",
    "believe it or not",
    "let me tell you",
    "the reason",
    "how to",
    "why ",
    "tahu nggak",
    "tahu gak",
    "tahukah",
    "tahu tidak",
    "pernah nggak",
    "pernah gak",
    "ternyata",
    "rahasia",
    "jangan ",
    "kenapa",
    "mengapa",
    "bagaimana",
    "gimana",
    "cara ",
    "ini dia",
    "yang paling",
    "terbesar",
    "terburuk",
    "terbaik",
    "percaya nggak",
    "percaya gak",
    "kalian harus",
    "dengerin",
    "dengarkan",
    "bayangkan",
    "bayangin",
];

/// Words that say nothing, which a clip should have few of.
const FILLERS: &[&str] = &[
    "um", "uh", "umm", "uhh", "hmm", "mmm", "eh", "ehm", "emm", "erm", "like", "you know", "anu",
    "apa", "ya", "kan", "tuh", "gitu", "sih", "deh", "lah", "nah", "oke", "okay", "ok",
];

/// Words that begin a sentence by leaning on the one before it: a clip that
/// opens on one starts mid-thought.
const LEANERS: &[&str] = &[
    "and", "but", "so", "because", "which", "that", "then", "also", "or", "dan", "tapi", "jadi",
    "karena", "yang", "terus", "lalu", "atau", "makanya", "soalnya", "sedangkan", "namun",
];

/// The words of `text`, lower-cased and stripped of punctuation.
fn words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| {
            word.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')
                .to_lowercase()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

/// Whether `text` ends a sentence.
fn ends_sentence(text: &str) -> bool {
    text.trim_end()
        .trim_end_matches(['"', '\'', ')', '”', '’'])
        .ends_with(['.', '?', '!', '…', '。', '？', '！'])
}

/// What one candidate window holds, summed as it grows.
#[derive(Default, Clone)]
struct Tally {
    words: usize,
    questions: usize,
    exclaims: usize,
    numbers: usize,
    fillers: usize,
}

impl Tally {
    fn add(&mut self, line: &Line) {
        let list = words(&line.text);
        self.words += list.len();
        self.questions += line.text.matches(['?', '？']).count();
        self.exclaims += line.text.matches(['!', '！']).count();
        self.numbers += list
            .iter()
            .filter(|word| word.chars().any(|c| c.is_ascii_digit()))
            .count();
        self.fillers += list.iter().filter(|word| FILLERS.contains(&word.as_str())).count();
    }
}

/// The mean of `energy` over `from..to` seconds (one value a second), or
/// none when the sound is not known there.
fn mean_energy(energy: &[f32], from: f64, to: f64) -> Option<f64> {
    if energy.is_empty() {
        return None;
    }
    let a = (from.max(0.0).floor() as usize).min(energy.len() - 1);
    let b = (to.max(0.0).ceil() as usize).clamp(a + 1, energy.len());
    let slice = &energy[a..b];
    Some(slice.iter().map(|&e| f64::from(e)).sum::<f64>() / slice.len() as f64)
}

/// How well `lines[first..=last]` would stand as a clip: 0 to 1.
fn score(
    lines: &[Line],
    first: usize,
    last: usize,
    tally: &Tally,
    energy: &[f32],
    baseline: Option<(f64, f64)>,
    opts: &Options,
) -> f64 {
    let (start, end) = (lines[first].start, lines[last].end);
    let length = (end - start).max(0.1);

    // pace: how close the speaking rate is to what holds a viewer
    let rate = tally.words as f64 / length;
    let pace = 1.0 - ((rate - IDEAL_RATE).abs() / IDEAL_RATE).min(1.0);

    // the hook: a question or a known opening in the first line or two
    let head: String = lines[first..=last.min(first + 1)]
        .iter()
        .map(|line| line.text.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    let opens_with_hook = HOOKS.iter().any(|hook| head.contains(hook));
    let opens_with_question = head.contains('?') || head.contains('？');
    let hook = if opens_with_hook {
        1.0
    } else if opens_with_question {
        0.8
    } else {
        0.0
    };

    // punch: questions, exclamations and figures, per ten words
    let per_ten = 10.0 / tally.words.max(1) as f64;
    let punch = ((tally.questions as f64 * 0.8
        + tally.exclaims as f64 * 0.8
        + tally.numbers as f64 * 0.4)
        * per_ten)
        .min(1.0);

    // variety: distinct words among the first fifty, filler counted against
    let all: Vec<String> = lines[first..=last]
        .iter()
        .flat_map(|line| words(&line.text))
        .take(50)
        .collect();
    let distinct: std::collections::HashSet<&String> = all.iter().collect();
    let variety = if all.is_empty() {
        0.0
    } else {
        distinct.len() as f64 / all.len() as f64
    };
    let filler = (tally.fillers as f64 / tally.words.max(1) as f64 * 4.0).min(1.0);

    // a clean start and a clean end
    let first_word = words(&lines[first].text).into_iter().next().unwrap_or_default();
    let leans = LEANERS.contains(&first_word.as_str());
    let clean_start = if leans { 0.0 } else { 1.0 };
    let clean_end = if ends_sentence(&lines[last].text) { 1.0 } else { 0.0 };

    // loudness against the talk as a whole, when the sound is known
    let loud = match (mean_energy(energy, start, end), baseline) {
        (Some(mean), Some((mid, spread))) if spread > 1e-6 => {
            ((mean - mid) / spread * 0.5 + 0.5).clamp(0.0, 1.0)
        }
        _ => 0.5,
    };

    // length: near the middle of what was asked for
    let target = (opts.min_len + opts.max_len) / 2.0;
    let half = ((opts.max_len - opts.min_len) / 2.0).max(1.0);
    let fit = 1.0 - ((length - target).abs() / half).min(1.0);

    let weights = [
        (hook, 0.22),
        (pace, 0.16),
        (punch, 0.14),
        (variety, 0.10),
        (1.0 - filler, 0.08),
        (clean_start, 0.08),
        (clean_end, 0.08),
        (loud, 0.08),
        (fit, 0.06),
    ];
    weights.iter().map(|(value, weight)| value * weight).sum::<f64>()
}

/// The middle and spread of the sound's level, to measure a window against.
fn baseline(energy: &[f32]) -> Option<(f64, f64)> {
    if energy.len() < 4 {
        return None;
    }
    let mut sorted: Vec<f64> = energy.iter().map(|&e| f64::from(e)).collect();
    sorted.sort_by(f64::total_cmp);
    let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
    Some((at(0.5), at(0.9) - at(0.1)))
}

/// Every window of whole lines inside one scene that is at least
/// `opts.min_len` and at most `opts.max_len` long, scored.
fn windows(lines: &[Line], energy: &[f32], opts: &Options) -> Vec<Pick> {
    let base = baseline(energy);
    let mut found = Vec::new();
    for first in 0..lines.len() {
        let mut tally = Tally::default();
        for last in first..lines.len() {
            // a long silence ends the scene: no clip crosses it
            if last > first && lines[last].start - lines[last - 1].end > opts.max_gap {
                break;
            }
            let length = lines[last].end - lines[first].start;
            if length > opts.max_len {
                break;
            }
            tally.add(&lines[last]);
            if length < opts.min_len {
                continue;
            }
            // a clip may stop in the middle of a sentence only when the talk
            // goes on without a pause there: otherwise it ends at a sentence
            // end or before a real pause
            let next_gap = lines
                .get(last + 1)
                .map_or(f64::INFINITY, |next| next.start - lines[last].end);
            if !ends_sentence(&lines[last].text) && next_gap < 0.6 {
                continue;
            }
            found.push(Pick {
                start: lines[first].start,
                end: lines[last].end,
                score: score(lines, first, last, &tally, energy, base, opts),
                first,
                last,
            });
        }
    }
    found
}

/// The best `opts.count` clips of `lines` (any order, but time-sorted
/// within the sound), best first, none overlapping another. `energy` is the
/// sound's level once a second from the start of the timeline, or empty.
pub fn select(lines: &[Line], energy: &[f32], opts: &Options) -> Vec<Pick> {
    let mut lines: Vec<Line> = lines
        .iter()
        .filter(|line| line.end > line.start && !line.text.trim().is_empty())
        .cloned()
        .collect();
    lines.sort_by(|a, b| a.start.total_cmp(&b.start));
    if lines.is_empty() || opts.count == 0 {
        return Vec::new();
    }

    // when nothing is long enough, ask for less and less down to five seconds
    let mut opts = *opts;
    let mut found = windows(&lines, energy, &opts);
    while found.is_empty() && opts.min_len > 5.0 {
        opts.min_len = (opts.min_len / 2.0).max(5.0);
        found = windows(&lines, energy, &opts);
    }
    if found.is_empty() {
        // a short talk is one clip
        let (first, last) = (0, lines.len() - 1);
        let mut tally = Tally::default();
        lines.iter().for_each(|line| tally.add(line));
        let value = score(&lines, first, last, &tally, energy, baseline(energy), &opts);
        found.push(Pick {
            start: lines[first].start,
            end: lines[last].end,
            score: value,
            first,
            last,
        });
    }

    found.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.start.total_cmp(&b.start)));
    let mut chosen: Vec<Pick> = Vec::new();
    for candidate in found {
        if chosen.len() >= opts.count {
            break;
        }
        if chosen
            .iter()
            .all(|kept| candidate.end <= kept.start || candidate.start >= kept.end)
        {
            chosen.push(candidate);
        }
    }

    // A breath either side, but at most half the silence to the neighbouring
    // line: the next clip, if it starts there, takes the other half, so two
    // clips side by side never overlap and no clip takes in a neighbour's words.
    for pick in &mut chosen {
        let room_before = pick
            .first
            .checked_sub(1)
            .map_or(pick.start, |i| (pick.start - lines[i].end).max(0.0) / 2.0);
        let room_after = lines
            .get(pick.last + 1)
            .map_or(f64::INFINITY, |line| (line.start - pick.end).max(0.0) / 2.0);
        pick.start = (pick.start - LEAD.min(room_before)).max(0.0);
        pick.end += TAIL.min(room_after);
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line of `text` said from `start` to `end`.
    fn say(start: f64, end: f64, text: &str) -> Line {
        Line {
            start,
            end,
            text: text.to_owned(),
        }
    }

    /// A run of lines every `step` seconds, each `step - 0.2` long.
    fn run(from: f64, count: usize, step: f64, text: impl Fn(usize) -> String) -> Vec<Line> {
        (0..count)
            .map(|i| {
                let start = from + i as f64 * step;
                say(start, start + step - 0.2, &text(i))
            })
            .collect()
    }

    fn opts() -> Options {
        Options {
            min_len: 10.0,
            max_len: 30.0,
            count: 3,
            max_gap: 2.5,
        }
    }

    #[test]
    fn nothing_said_picks_nothing() {
        assert!(select(&[], &[], &opts()).is_empty());
        assert!(select(&[say(0.0, 1.0, "   ")], &[], &opts()).is_empty());
    }

    #[test]
    fn a_hooked_dense_stretch_beats_a_slow_filler_one() {
        // 0-24 s: filler, slow; 40-64 s: a hook, then dense talk with questions
        let mut lines = run(0.0, 6, 4.0, |_| "um eh yeah so uh".to_owned());
        lines.extend(run(40.0, 6, 4.0, |i| {
            if i == 0 {
                "Did you know the secret to editing fast is hidden in the timeline?".to_owned()
            } else if i == 5 {
                "That is why 3 simple cuts beat 30 clumsy ones.".to_owned()
            } else {
                format!("Every single clip number {i} carries its own rhythm and its own story?")
            }
        }));
        let picks = select(&lines, &[], &Options { count: 1, ..opts() });
        assert_eq!(picks.len(), 1);
        assert!(picks[0].start >= 39.0, "{picks:?}");
    }

    #[test]
    fn no_clip_crosses_a_long_silence() {
        // two scenes of 12 s with a 10 s silence: no clip may hold both
        let mut lines = run(0.0, 4, 3.0, |i| format!("First scene sentence number {i} here."));
        lines.extend(run(22.0, 4, 3.0, |i| format!("Second scene sentence number {i} here.")));
        for pick in select(&lines, &[], &Options { count: 5, ..opts() }) {
            assert!(
                pick.end <= 12.5 || pick.start >= 21.0,
                "crosses the gap: {pick:?}"
            );
        }
    }

    #[test]
    fn clips_never_overlap_and_keep_to_their_length() {
        let lines = run(0.0, 60, 3.0, |i| format!("Sentence {i} about something quite interesting."));
        let o = Options { count: 4, ..opts() };
        let picks = select(&lines, &[], &o);
        assert!(picks.len() >= 2);
        for (i, a) in picks.iter().enumerate() {
            assert!(a.end - a.start <= o.max_len + LEAD + TAIL + 1e-6, "{a:?}");
            assert!(a.end - a.start >= o.min_len, "{a:?}");
            for b in &picks[i + 1..] {
                assert!(a.end <= b.start + 1e-9 || b.end <= a.start + 1e-9, "{a:?} {b:?}");
            }
        }
        // best first
        assert!(picks.windows(2).all(|w| w[0].score >= w[1].score));
    }

    #[test]
    fn a_short_talk_is_one_clip() {
        let lines = vec![say(0.0, 3.0, "Hello there."), say(3.2, 6.0, "This is short.")];
        let picks = select(&lines, &[], &opts());
        assert_eq!(picks.len(), 1);
        assert!(picks[0].start <= 0.01 && picks[0].end >= 6.0);
    }

    #[test]
    fn a_clip_starts_a_breath_before_the_first_word_and_stays_clear_of_the_last_line() {
        let lines = run(5.0, 6, 4.0, |i| format!("Line {i} says something worth hearing."));
        let picks = select(&lines, &[], &Options { count: 1, ..opts() });
        let p = &picks[0];
        let first = &lines[p.first];
        assert!(p.start < first.start && first.start - p.start <= LEAD + 1e-9);
        if p.first > 0 {
            assert!(p.start >= lines[p.first - 1].end - 1e-9);
        }
        if let Some(next) = lines.get(p.last + 1) {
            assert!(p.end <= next.start + 1e-9);
        }
    }

    #[test]
    fn the_louder_stretch_wins_when_the_talk_is_alike() {
        // the same kind of talk across 120 s, loud only in 60-90 s
        let lines = run(0.0, 40, 3.0, |i| format!("Sentence {i} about something quite interesting."));
        let energy: Vec<f32> = (0..120)
            .map(|s| if (60..90).contains(&s) { -12.0 } else { -40.0 })
            .collect();
        let picks = select(&lines, &energy, &Options { count: 1, ..opts() });
        let p = &picks[0];
        assert!(p.start >= 55.0 && p.end <= 95.0, "{p:?}");
    }

    #[test]
    fn indonesian_hooks_count() {
        let mut lines = run(0.0, 6, 4.0, |i| format!("Kalimat biasa nomor {i} tentang hal lain."));
        lines.extend(run(40.0, 6, 4.0, |i| {
            if i == 0 {
                "Tahu nggak, ternyata rahasia ini jarang ada yang sadar?".to_owned()
            } else {
                format!("Kalimat biasa nomor {i} tentang hal lain.")
            }
        }));
        let picks = select(&lines, &[], &Options { count: 1, ..opts() });
        assert!(picks[0].start >= 39.0, "{picks:?}");
    }

    #[test]
    fn a_clip_does_not_open_on_a_leaning_word() {
        // identical talk, but every line of the first scene starts with "and"
        let mut lines = run(0.0, 6, 4.0, |i| format!("And sentence {i} about something quite interesting."));
        lines.extend(run(40.0, 6, 4.0, |i| format!("Sentence {i} about something quite interesting.")));
        let picks = select(&lines, &[], &Options { count: 1, ..opts() });
        assert!(picks[0].start >= 39.0, "{picks:?}");
    }
}
