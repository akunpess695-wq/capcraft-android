// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 CapCraft contributors

//! Clipper: a long talk cut down to the few moments worth posting.
//!
//! The talk is transcribed on the phone ([`MobileSession::transcript`]),
//! [`MobileSession::suggest_clips`] picks the stretches of it most likely to
//! hold a viewer (see [`select`]; nothing leaves the phone), and
//! [`make_clip_project`] turns each pick the user keeps into a project of its
//! own: a copy of the source cut down to that stretch, a tall 9:16 frame
//! with the picture fitted over a blurred fill of itself, and the words
//! under it as captions. Each project then opens in the editor like any
//! other, to be changed and exported.

mod select;

use concat_media::{AudioDecoder, AudioOptions, SampleFormat};
use concat_project::model::{ClipKind, MediaKind};

use crate::library::{ProjectSummary, delete_project, duplicate_project, project_summary};
use crate::loudness::level;
use crate::session::MobileSession;
use crate::speech::{TranscriptLine, chunks};
use crate::{MobileError, micros, seconds};

/// The rate the sound is read at for its level.
const RATE: u32 = 8_000;
/// The level given to a second with nothing in it, dB of full scale.
const SILENT: f32 = -90.0;
/// How many characters of what is said a suggestion shows.
const EXCERPT: usize = 160;

/// A stretch of the timeline worth making a clip of.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ClipSuggestion {
    /// Where it starts, microseconds into the timeline.
    pub start_us: i64,
    /// Where it ends.
    pub end_us: i64,
    /// How well it scored, 0 to 100.
    pub score: u32,
    /// What is said in it, the start of it.
    pub text: String,
}

/// The start of `text` cut to [`EXCERPT`] characters at a word, with an
/// ellipsis when something was left off.
fn excerpt(text: &str) -> String {
    if text.chars().count() <= EXCERPT {
        return text.to_owned();
    }
    let cut: String = text.chars().take(EXCERPT).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    format!("{}…", cut.trim_end())
}

impl MobileSession {
    /// How loud the main track's sound is, a value for each second of the
    /// timeline in dB of full scale: what the picker weighs a stretch's
    /// loudness by. Empty when nothing could be read; a clip whose sound
    /// cannot be opened is passed over rather than failing the choice.
    fn energy_per_second(&self) -> Vec<f32> {
        let clips = self
            .with_session(|session| {
                let project = session.project();
                let timeline = project.active();
                let Some(main) = timeline.tracks.first() else {
                    return Ok(Vec::new());
                };
                Ok(timeline
                    .clips
                    .iter()
                    .filter(|clip| {
                        clip.track_id == main.id
                            && clip.kind == ClipKind::Video
                            && clip.muted != Some(true)
                    })
                    .filter_map(|clip| {
                        let media = project.media.iter().find(|item| item.id == clip.media_id)?;
                        (media.has_audio && !matches!(media.kind, MediaKind::Image)).then(|| {
                            (
                                clip.start,
                                clip.speed.max(1e-6),
                                media.path.clone(),
                                clip.source_start,
                                clip.duration * clip.speed,
                                clip.audio_stream.map(|index| index as usize),
                            )
                        })
                    })
                    .collect::<Vec<_>>())
            })
            .unwrap_or_default();
        let length = seconds(self.view().duration_us).ceil().max(0.0) as usize + 1;
        let mut energy = vec![SILENT; length];
        for (start, speed, path, from, span, stream) in clips {
            let Ok(mut decoder) = AudioDecoder::open(
                &path,
                &AudioOptions {
                    start: Some(from),
                    duration: Some(span.max(0.05)),
                    filters: Vec::new(),
                    rate: RATE,
                    channels: 1,
                    format: SampleFormat::F32,
                    stream,
                },
            ) else {
                continue;
            };
            let Ok(samples) = decoder.collect_f32() else {
                continue;
            };
            // a second of the file is 1 / speed seconds of the timeline
            for (index, chunk) in samples.chunks(RATE as usize).enumerate() {
                let at = (start + index as f64 / speed).floor() as usize;
                if let Some(slot) = energy.get_mut(at) {
                    *slot = level(chunk, RATE).map_or(SILENT, |db| db as f32);
                }
            }
        }
        energy
    }
}

#[uniffi::export]
impl MobileSession {
    /// The stretches of the talk best made into clips of `min_us` to
    /// `max_us`: at most `count`, best first, none overlapping, each opening
    /// and closing on what is said. `lines` is the talk's transcript (from
    /// [`MobileSession::transcript`]). Errs when there is nothing said.
    pub fn suggest_clips(
        &self,
        lines: Vec<TranscriptLine>,
        min_us: i64,
        max_us: i64,
        count: u32,
    ) -> Result<Vec<ClipSuggestion>, MobileError> {
        let said: Vec<select::Line> = lines
            .iter()
            .map(|line| select::Line {
                start: seconds(line.start_us),
                end: seconds(line.end_us),
                text: line.text.clone(),
            })
            .collect();
        if said.iter().all(|line| line.text.trim().is_empty()) {
            return Err(MobileError::Failed(
                "There is no speech to choose clips from.".to_owned(),
            ));
        }
        let min_len = seconds(min_us).max(5.0);
        let options = select::Options {
            min_len,
            max_len: seconds(max_us).max(min_len + 1.0),
            count: count.clamp(1, 20) as usize,
            ..select::Options::default()
        };
        let total = self.view().duration_us;
        let energy = self.energy_per_second();
        Ok(select::select(&said, &energy, &options)
            .into_iter()
            .map(|pick| {
                let (start_us, end_us) = (micros(pick.start), micros(pick.end).min(total));
                let text = lines
                    .iter()
                    .filter(|line| line.start_us >= start_us - 300_000 && line.end_us <= end_us + 300_000)
                    .map(|line| line.text.trim())
                    .collect::<Vec<_>>()
                    .join(" ");
                ClipSuggestion {
                    start_us,
                    end_us,
                    score: (pick.score * 100.0).round().clamp(0.0, 100.0) as u32,
                    text: excerpt(&text),
                }
            })
            .collect())
    }
}

/// Makes the clip `start_us..end_us` of the project in `source_path` into a
/// project of its own titled `name`: a copy of the source cut down to that
/// stretch, framed 9:16 with a blurred fill, its words under it as captions
/// (`lines` is the source's transcript). The new project is saved and
/// returned; the source is left as it was. When any step fails, the copy is
/// removed.
#[uniffi::export]
pub fn make_clip_project(
    source_path: String,
    name: String,
    start_us: i64,
    end_us: i64,
    lines: Vec<TranscriptLine>,
) -> Result<ProjectSummary, MobileError> {
    if end_us <= start_us || start_us < 0 {
        return Err(MobileError::Failed(
            "That clip has no length.".to_owned(),
        ));
    }
    let made = duplicate_project(source_path, name)?;
    let build = || -> Result<(), MobileError> {
        let session = MobileSession::open(made.path.clone())?;
        let total = session.view().duration_us;
        let end_us = end_us.min(total);
        if end_us <= start_us {
            return Err(MobileError::Failed(
                "That clip lies past the end of the video.".to_owned(),
            ));
        }
        // the tail first, so the head's time is where it was
        if end_us < total {
            session.cut_range(end_us, total)?;
        }
        if start_us > 0 {
            session.cut_range(0, start_us)?;
        }
        session.set_canvas(1080, 1920)?;
        session.set_background(r#"{"kind":"blur","level":2}"#.to_owned())?;
        let captions: Vec<(f64, f64, String)> = lines
            .iter()
            .filter(|line| line.end_us > start_us && line.start_us < end_us)
            .flat_map(|line| {
                let from = seconds(line.start_us.max(start_us) - start_us);
                let to = seconds(line.end_us.min(end_us) - start_us);
                chunks(from, to, line.text.trim())
            })
            .filter(|(_, _, text)| !text.is_empty())
            .collect();
        if !captions.is_empty() {
            session.place_captions(&captions, false)?;
        }
        session.save()
    };
    match build() {
        Ok(()) => project_summary(made.path),
        Err(error) => {
            // no half-made project is left in the library
            let _ = delete_project(made.path);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_text_is_shown_whole() {
        assert_eq!(excerpt("Short and sweet."), "Short and sweet.");
    }

    #[test]
    fn a_long_text_is_cut_at_a_word_with_an_ellipsis() {
        let text = "word ".repeat(100);
        let shown = excerpt(&text);
        assert!(shown.ends_with('…'));
        assert!(shown.chars().count() <= EXCERPT + 1);
        assert!(!shown.trim_end_matches('…').ends_with(' '));
    }
}
