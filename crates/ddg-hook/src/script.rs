//! Scripted inputs for the oracle: `tick notch brake` lines, piecewise constant.
//!
//! `tick` counts driving ticks (from the first tick of the run the hook sees). From each line's
//! tick on, power notch and brake notch are held at the given values until the next line.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub tick: u32,
    pub notch: i32,
    pub brake: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Script {
    steps: Vec<Step>,
}

impl Script {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut steps = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let v: Vec<&str> = line.split_whitespace().collect();
            let [tick, notch, brake] = v[..] else {
                return Err(format!("line {}: expected `tick notch brake`", n + 1));
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
            if steps.last().is_some_and(|p: &Step| p.tick > step.tick) {
                return Err(format!("line {}: ticks must not decrease", n + 1));
            }
            steps.push(step);
        }
        Ok(Self { steps })
    }

    /// The inputs held at driving tick `tick`, if the script has started.
    pub fn at(&self, tick: u32) -> Option<(i32, i32)> {
        let i = self.steps.partition_point(|s| s.tick <= tick);
        let s = self.steps.get(i.checked_sub(1)?)?;
        Some((s.notch, s.brake))
    }

    /// Driving tick after which the script has no more changes.
    pub fn last_tick(&self) -> Option<u32> {
        self.steps.last().map(|s| s.tick)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn piecewise_constant() {
        let s = Script::parse("0 0 8\n30 5 0 # full power\n300 0 9\n").unwrap();
        assert_eq!(s.at(0), Some((0, 8)));
        assert_eq!(s.at(29), Some((0, 8)));
        assert_eq!(s.at(30), Some((5, 0)));
        assert_eq!(s.at(1000), Some((0, 9)));
        assert_eq!(Script::parse("5 0 0").unwrap().at(4), None);
        assert!(Script::parse("0 6 0").is_err());
        assert!(Script::parse("10 0 0\n5 0 0").is_err());
    }
}
