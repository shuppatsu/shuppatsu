//! A record of the voice lines the game starts, for subtitles (a port addition: the original
//! keeps no such record). [`crate::SoundSystem`] appends a [`Started`] for every single sound
//! it plays through `snd_request` and every sequence it starts, with the call that built it,
//! so a reader can tell "station name + suffix" from "signal kind + aspect" without guessing
//! from the codes alone (docs/localisation.md, "Subtitles").

use crate::system::SEQUENCE_TICKS_PER_FRAME;
use crate::tables::Step;

/// The call that started a voice line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// `snd_request(code)` of a single sound (0x41fda0).
    Code(u16),
    /// Table sequence `id` (`snd_request(0x1000 | id)`).
    Sequence(usize),
    /// [`crate::SoundSystem::call_pair`]: pair `second`, then pair `first` (0x41ffe0).
    Pair { first: usize, second: usize },
    /// [`crate::SoundSystem::call_prefix_speed`] (0x420180).
    PrefixSpeed { prefix: usize, kmh: i32 },
    /// [`crate::SoundSystem::call_signal`] (0x420320).
    Signal { prefix: i32, mode: i32, aspect: i32 },
    /// [`crate::SoundSystem::call_station`] (0x420040), `name` = station-name id.
    Station { mode: i32, name: usize },
    /// Any other caller-built list ([`crate::SoundSystem::start_sequence_list`]).
    List,
}

/// One voice line as it was started: its origin and the steps it plays (one step for a
/// single sound, delay 0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    pub origin: Origin,
    pub steps: Vec<Step>,
}

/// What the game thread did to the voices, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceEvent {
    Started(Started),
    /// Stop-all (0x420660) or the sequencer switched off (0x84faf8 = 0): what was being said
    /// is cut off.
    Silenced,
}

/// When each step of a sequence starts, in sequencer ticks after the start (0x41ff60): step 0
/// at once, step 1 after `delay₀ + 1`, every later one `max(delayₖ, 1)` after the previous
/// (docs/re/audio-text.md §5). Steps after one whose successor ends the list are not played,
/// so the result has one entry per step of `steps` up to the first `delay == −1` step.
pub fn step_offsets(steps: &[Step]) -> Vec<u32> {
    let mut out = Vec::with_capacity(steps.len());
    let mut t = 0u32;
    for (i, s) in steps.iter().enumerate() {
        if s.code == -1 || (i > 0 && s.delay == -1) {
            break;
        }
        out.push(t);
        let d = u32::try_from(s.delay).unwrap_or(0);
        t += if i == 0 { d + 1 } else { d.max(1) };
    }
    out
}

/// Sequencer ticks to game ticks, rounded up (two sequencer ticks per 33 ms game tick).
pub fn game_ticks(sequencer_ticks: u32) -> u32 {
    sequencer_ticks.div_ceil(SEQUENCE_TICKS_PER_FRAME as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_follow_the_sequencer() {
        let s = [Step::new(1, 2), Step::new(2, 0), Step::new(3, 5), Step::END];
        assert_eq!(step_offsets(&s), [0, 3, 4]);
        // A step whose delay is −1 ends the sequence and is not played.
        let s = [Step::new(1, 0), Step::new(2, -1)];
        assert_eq!(step_offsets(&s), [0]);
        assert!(step_offsets(&[]).is_empty());
        assert_eq!(game_ticks(3), 2);
        assert_eq!(game_ticks(4), 2);
    }
}
