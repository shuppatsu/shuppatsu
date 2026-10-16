//! The attract demo drives (docs/re/menus.md §2): 0x40e4b0 drives the demo route for 1600
//! ticks, 0x40e6e0 drives stage 5 with its lesson course events for 3250 ticks. In both the
//! player-train task runs in its replay mode (0x4229c0 with mode 2 or 3 → task+0x74 = 2): every
//! update reads one byte of an input table in the exe instead of the controls.
//!
//! An input byte is `brake << 4 | horn << 3 | power` (0x422c71: `shr 4`, `and 7`, `test 8`).
//! The route demo reads 0x467b20, the lesson demo 0x46857c; the end-of-table test always reads
//! 0x467b20 (`cmp byte [edi + 0x467b20], 0xff`), so for the lesson demo it looks at the bytes
//! after the first table, which hold no 0xff within the demo's length.

use ddg_formats::Result;
use ddg_formats::exe::Exe;
use ddg_formats::stages::{CourseEvent, read_course_events};

use crate::drive::DriverInput;

/// The route demo's input table (0x467b20) and the lesson demo's (0x46857c, right after it).
const ROUTE_INPUTS: u32 = 0x467b20;
const LESSON_INPUTS: u32 = 0x46857c;
/// The lesson table is followed by other data at 0x469290; its tail is zero padding.
const INPUTS_END: u32 = 0x469290;
/// The lesson demo's course events (0x40e6e0: `0x857088 = 0x459338`).
const LESSON_COURSE: u32 = 0x459338;
/// End-of-table marker of the input table.
const END_MARK: u8 = 0xff;

/// Which demo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemoKind {
    /// 0x40e4b0, task 0x40e630: 0x40dad0 and the AI-train events (0x40b710) only.
    Route,
    /// 0x40e6e0 (stage 5), task 0x40e860: scripted hints and signals, then 0x40dad0, the
    /// course events, the signals and the speed enforcement.
    Lesson,
}

impl DemoKind {
    /// The task runs while its counter is below this (0x40e630: 0x640, 0x40e860: 0xcb2).
    pub fn ticks(self) -> u32 {
        match self {
            Self::Route => 0x640,
            Self::Lesson => 0xcb2,
        }
    }

    /// The stage the lesson demo always uses (0x40e6e0: `0x8570a4 = 5`).
    pub const LESSON_STAGE: usize = 5;
}

/// The exe data of the demos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DemoTables {
    /// Both input tables, contiguous from 0x467b20 (the end test indexes this from the start).
    inputs: Vec<u8>,
    lesson_offset: usize,
    pub lesson_course: Vec<CourseEvent>,
}

impl DemoTables {
    pub fn read(exe: &Exe<'_>) -> Result<Self> {
        let inputs = exe
            .bytes_at(ROUTE_INPUTS, (INPUTS_END - ROUTE_INPUTS) as usize)?
            .to_vec();
        Ok(Self {
            inputs,
            lesson_offset: (LESSON_INPUTS - ROUTE_INPUTS) as usize,
            lesson_course: read_course_events(exe, LESSON_COURSE)?,
        })
    }
}

/// The replay state of the train task (task+0x70 index, task+0x68 horn held).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DemoInputs {
    index: usize,
    horn: bool,
}

impl DemoInputs {
    /// One train-task update: the notches and horn of this tick.
    pub fn next(&mut self, t: &DemoTables, kind: DemoKind) -> DriverInput {
        let base = match kind {
            DemoKind::Route => 0,
            DemoKind::Lesson => t.lesson_offset,
        };
        if t.inputs.get(self.index).copied().unwrap_or(END_MARK) == END_MARK {
            self.index = 0;
            return DriverInput {
                horn_held: self.horn,
                ..DriverInput::default()
            };
        }
        let b = t.inputs.get(base + self.index).copied().unwrap_or(0);
        self.index += 1;
        let horn = b & 8 != 0;
        let pressed = horn && !self.horn;
        self.horn = horn;
        DriverInput {
            power: b & 7,
            brake: b >> 4,
            horn_held: horn,
            horn_pressed: pressed,
            confirm_pressed: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_decode_into_notches_and_horn() {
        let t = DemoTables {
            inputs: vec![0x05, 0x48, 0x48, 0x00, 0x13, 0xff, 0x00],
            lesson_offset: 3,
            lesson_course: Vec::new(),
        };
        let mut d = DemoInputs::default();
        let r: Vec<_> = (0..6)
            .map(|_| {
                let i = d.next(&t, DemoKind::Route);
                (i.power, i.brake, i.horn_held, i.horn_pressed)
            })
            .collect();
        assert_eq!(
            r,
            [
                (5, 0, false, false),
                (0, 4, true, true),
                (0, 4, true, false),
                (0, 0, false, false),
                (3, 1, false, false),
                // The end mark: no notches, back to the start.
                (0, 0, false, false),
            ]
        );
        let mut d = DemoInputs::default();
        assert_eq!(d.next(&t, DemoKind::Lesson).power, 0);
        assert_eq!(d.next(&t, DemoKind::Lesson).power, 3);
    }
}
