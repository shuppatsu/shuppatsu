//! The AI-train ("traffic") model table of the exe: per train type, the cars of the train for
//! each running direction, with five level-of-detail parts per car and the car spacing.
//!
//! Read at runtime from the user's exe; layout and evidence in `docs/re/traffic.md` §2.

use crate::error::{FormatError, Result};
use crate::exe::Exe;

const WHAT: &str = "traffic model table";

/// `{car list for direction 0, car list for direction 1, i32 kind}` per row, 12 bytes
/// (0x404f10 reads `0x457358[row·3 + dir]` and the kind at 0x457360).
pub const MODEL_TABLE: u32 = 0x457358;
/// Rows 0..=78. Row 79 would start at 0x4576ac, which holds unrelated doubles.
pub const MODEL_ROWS: usize = 79;
/// One car record: five part ids (LOD 0..4), f32 offset from the head, f32 spacing to the car
/// before; a record whose first part id is 0 ends the list.
const CAR_LEN: u32 = 28;
/// Sanity bound: the longest list in the exe has 16 cars.
const MAX_CARS: usize = 64;

/// One car of an AI train.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CarModel {
    /// Part ids for the five distance bands (0x405640 picks one per frame).
    pub lods: [u32; 5],
    /// f32 at +20: distance of this car behind the head of the train, world units (≤ 0).
    pub offset: f32,
    /// f32 at +24: distance to the car before, world units (≤ 0; 0 for the first car).
    pub spacing: f32,
}

/// One row of the table: a train type.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrainModel {
    /// The i32 at +8. 0 = road vehicles (area parts), 1..=8 = rail; kinds 3, 7 and 8 are
    /// spawned with the *other* direction's list in reverse order (0x404ea0).
    pub kind: i32,
    /// Car lists by direction flag (0 = runs towards the start of its segment, 1 = towards
    /// the end). Empty where the exe has a null pointer.
    pub cars: [Vec<CarModel>; 2],
}

/// The whole table.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrainModels {
    pub rows: Vec<TrainModel>,
}

impl TrainModels {
    pub fn read(exe: &Exe<'_>) -> Result<Self> {
        let rows = (0..MODEL_ROWS as u32)
            .map(|row| {
                let base = MODEL_TABLE + 12 * row;
                let r = exe.reader_at(base, 12, WHAT)?;
                Ok(TrainModel {
                    kind: r.i32(8)?,
                    cars: [read_cars(exe, r.u32(0)?)?, read_cars(exe, r.u32(4)?)?],
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self { rows })
    }

    pub fn row(&self, row: usize) -> Option<&TrainModel> {
        self.rows.get(row)
    }
}

fn read_cars(exe: &Exe<'_>, va: u32) -> Result<Vec<CarModel>> {
    let mut cars = Vec::new();
    if va == 0 {
        return Ok(cars);
    }
    for i in 0..=MAX_CARS as u32 {
        let r = exe.reader_at(va + CAR_LEN * i, CAR_LEN as usize, WHAT)?;
        if r.u32(0)? == 0 {
            return Ok(cars);
        }
        let mut lods = [0u32; 5];
        for (k, l) in lods.iter_mut().enumerate() {
            *l = r.u32(4 * k)?;
        }
        cars.push(CarModel {
            lods,
            offset: r.f32(20)?,
            spacing: r.f32(24)?,
        });
    }
    Err(FormatError::invalid(
        WHAT,
        format!("car list at {va:#x} has no terminator"),
    ))
}
