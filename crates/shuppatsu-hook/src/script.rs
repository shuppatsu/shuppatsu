//! Scripted inputs for the oracle: `tick notch brake` lines, piecewise constant.
//!
//! `tick` counts driving ticks from the first tick of a leg (one station to the next) the hook
//! sees; the count starts again at 0 when the next leg starts driving. From each line's tick on,
//! power notch and brake notch are held at the given values until the next line.
//!
//! A line `leg` ends the current leg's lines: the lines after it drive the next leg. A script
//! without `leg` lines drives every leg the same way. A leg after the script's last section
//! has no inputs (the levers stay as they are).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub tick: u32,
    pub notch: i32,
    pub brake: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Script {
    /// The steps of each leg, in leg order.
    legs: Vec<Vec<Step>>,
    /// No `leg` line: the one section drives every leg.
    every_leg: bool,
}

impl Script {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut legs = vec![Vec::new()];
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if line == "leg" {
                legs.push(Vec::new());
                continue;
            }
            let v: Vec<&str> = line.split_whitespace().collect();
            let [tick, notch, brake] = v[..] else {
                return Err(format!(
                    "line {}: expected `tick notch brake` or `leg`",
                    n + 1
                ));
            };
            let num = |s: &str| {
                s.parse::<i64>()
                    .map_err(|e| format!("line {}: {s}: {e}", n + 1))
            };
            let step = Step {
                tick: u32::try_from(num(tick)?).map_err(|e| format!("line {}: {e}", n + 1))?,
                notch: num(notch)? as i32,
                brake: num(brake)? as i32,
            };
            if !(0..=5).contains(&step.notch) || !(0..=10).contains(&step.brake) {
                return Err(format!("line {}: notch 0..=5, brake 0..=10", n + 1));
            }
            let steps = legs.last_mut().expect("at least one leg");
            if steps.last().is_some_and(|p: &Step| p.tick > step.tick) {
                return Err(format!("line {}: ticks must not decrease", n + 1));
            }
            steps.push(step);
        }
        let every_leg = legs.len() == 1;
        Ok(Self { legs, every_leg })
    }

    /// The inputs held at driving tick `tick` of leg `leg` (0 = the first leg the hook sees),
    /// if the script has started.
    pub fn at(&self, leg: usize, tick: u32) -> Option<(i32, i32)> {
        let steps = self.legs.get(if self.every_leg { 0 } else { leg })?;
        let i = steps.partition_point(|s| s.tick <= tick);
        let s = steps.get(i.checked_sub(1)?)?;
        Some((s.notch, s.brake))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn piecewise_constant() {
        let s = Script::parse("0 0 8\n30 5 0 # full power\n300 0 9\n").unwrap();
        assert_eq!(s.at(0, 0), Some((0, 8)));
        assert_eq!(s.at(0, 29), Some((0, 8)));
        assert_eq!(s.at(0, 30), Some((5, 0)));
        assert_eq!(s.at(0, 1000), Some((0, 9)));
        assert_eq!(s.at(3, 30), Some((5, 0)), "one section drives every leg");
        assert_eq!(Script::parse("5 0 0").unwrap().at(0, 4), None);
        assert!(Script::parse("0 6 0").is_err());
        assert!(Script::parse("10 0 0\n5 0 0").is_err());
    }

    #[test]
    fn legs_restart_the_ticks() {
        let s = Script::parse("0 0 8\n100 5 0\nleg\n0 0 9\n50 3 0\n").unwrap();
        assert_eq!(s.at(0, 120), Some((5, 0)));
        assert_eq!(s.at(1, 10), Some((0, 9)));
        assert_eq!(s.at(1, 60), Some((3, 0)));
        assert_eq!(s.at(2, 60), None, "no inputs after the last leg");
    }
}
