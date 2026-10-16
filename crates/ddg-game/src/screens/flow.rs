//! The stage flow around a run (`Stage_Run` 0x4080d0; menus.md §8–§11.4): when the pause,
//! station result, continue, report card and run-end screens open and close, with the
//! original's `Task_Tick` counts between them.
//!
//! The flow never touches the simulation. The game calls [`StageFlow::after_tick`] after every
//! `Drive::tick` with an [`Observation`] and carries out the returned [`Action`]s on the
//! `Drive`; a pause replaces simulation ticks by [`StageFlow::frozen_tick`].
//!
//! Tick gaps (from 0x4080d0, counted in ticks that end in the given phase):
//! * Result: created after the fifth tick ending in `Arrived` (the arrival tick, the two
//!   `Task_Tick`s before the odometer settles at 0x40869d, two more before 0x413d90); its first
//!   update is the next tick. `finish_station` follows one tick after the tick it closed in.
//! * Continue after a time-out while driving: 20 drain ticks (the game-over tick is the
//!   first), the `Task_Tick`s at 0x408685 and after the arrival branch, then 0x415ab0: created
//!   after 22 ticks. After a time-out at a station result: one tick after `finish_station`.
//! * Report card: four ticks after the continue screen declined (0x408839, 0x40883e, 0x4088ac
//!   and the one after the task lists are killed), three after the last `finish_station`, two
//!   after the continue point when none is offered.
//! * Ranking: the report card ends with [`Action::Ranking`] and the game answers with
//!   [`StageFlow::start_ranking`] in the same tick. 0x41e8d0 creates the name-entry task
//!   between `Task_Tick`s, so its first update is the next tick. The run-end banner is created
//!   in the tick the name entry ends, or at once when there is none; two ticks after the banner
//!   the stage returns to the title.

use super::continue_screen::{ContinueDefs, ContinueScreen};
use super::pause::{PauseDefs, PauseMenu, PauseResult};
use super::ranking::{NameEntry, RankingDefs, RankingRequest};
use super::report::{ReportCard, ReportDefs};
use super::result::{ResultDefs, ResultInput, StationResultScreen};
use super::run_end::{RunEndBanner, RunEndDefs, RunEndInput};
use super::tables::ScreenTables;
use super::{MenuInput, Sfx};
use crate::save::Table;
use ddg_engine::drive::DrivePhase;
use ddg_engine::rules::StationOutcome;
use ddg_render::sprites::{Slot, SpriteLayer};

const RESULT_AFTER_ARRIVED_TICKS: u32 = 5;
const CONTINUE_AFTER_GAME_OVER_TICKS: u32 = 22;
const REPORT_AFTER_NO_CONTINUE_OFFERED: u32 = 2;
const CONTINUE_AFTER_STATION_GAME_OVER: u32 = 1;
const REPORT_AFTER_CONTINUE_DECLINED: u32 = 4;
const REPORT_AFTER_LAST_STATION: u32 = 3;
const TITLE_AFTER_BANNER: u32 = 2;

/// Everything the screens need from the run, sampled after each simulation tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Observation {
    pub phase: DrivePhase,
    pub can_continue: bool,
    pub remaining_s: i32,
    /// The station result's inputs, valid in `Arrived`.
    pub result: ResultInput,
    /// Report-card grade parts (`Rules::grades`).
    pub grades: [i32; 5],
    pub continues: i32,
    pub stage: usize,
    pub stage_kind: i32,
    /// The time-up popup (0x4158b0) is still running: `Stage_Run` waits on its phase flag 0x400
    /// before opening the continue screen (0x4080d0, the loop before 0x415ab0).
    pub time_up_running: bool,
}

/// What the game must do on the `Drive` (or the app) after a flow call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Call `Drive::finish_station`, then [`StageFlow::station_finished`].
    FinishStation,
    /// Call `Drive::continue_run`.
    ContinueRun,
    /// Pause → 始発駅に戻る: restart the stage from station 0 without the departure board.
    Restart,
    /// Pause → 運転中止: back to the title (no title screen yet: leave play).
    Quit,
    /// The report card is over: raise the clear mark if `cleared`, then call
    /// [`StageFlow::start_ranking`] with the ranking value `Rules::ranking_score(cleared)`
    /// (menus.md §11.4).
    Ranking { cleared: bool },
    /// The name entry ended: `rows` replace the in-memory ranking `table`; with `commit` the
    /// name was completed and the table is packed into the eeprom mirror (0x41e670).
    RankingDone {
        table: usize,
        rows: Table,
        commit: bool,
    },
    /// The run is over; back to the title.
    RunOver,
}

#[derive(Debug, Clone)]
pub struct FlowDefs {
    pub pause: PauseDefs,
    pub result: ResultDefs,
    pub continue_: ContinueDefs,
    pub report: ReportDefs,
    pub run_end: RunEndDefs,
    pub ranking: RankingDefs,
}

impl FlowDefs {
    pub fn new(t: &ScreenTables, map_size: impl Fn(i16) -> (i16, i16)) -> Self {
        Self {
            pause: PauseDefs::new(t, map_size),
            result: ResultDefs::new(t),
            continue_: ContinueDefs::new(t),
            report: ReportDefs::new(t),
            run_end: RunEndDefs::new(t),
            ranking: RankingDefs::new(t),
        }
    }
}

enum State {
    Run {
        arrived: u32,
        game_over: u32,
    },
    Paused(Box<PauseMenu>),
    Result(Box<StationResultScreen>),
    /// The result closed; `finish_station` runs after the next tick.
    AfterResult {
        remaining_s: Option<i32>,
        waited: bool,
    },
    ContinueWait(u32),
    Continue(Box<ContinueScreen>),
    EndWait {
        ticks: u32,
        cleared: bool,
    },
    Report {
        card: Box<ReportCard>,
        cleared: bool,
    },
    /// Waiting for [`StageFlow::start_ranking`], with the banner's inputs.
    RankingPending(RunEndInput),
    Ranking {
        entry: Box<NameEntry>,
        banner: RunEndInput,
    },
    RunEnd(Box<RunEndBanner>),
    Leaving(u32),
    Over,
}

pub struct StageFlow {
    defs: FlowDefs,
    state: State,
    /// Slots written by the last draw.
    drawn: Vec<usize>,
}

impl StageFlow {
    pub fn new(defs: FlowDefs) -> Self {
        Self {
            defs,
            drawn: Vec::new(),
            state: State::Run {
                arrived: 0,
                game_over: 0,
            },
        }
    }

    /// Back to driving after a restart.
    pub fn reset(&mut self) {
        self.state = State::Run {
            arrived: 0,
            game_over: 0,
        };
    }

    /// The pause dialogs are up: the next tick is a frozen one.
    pub fn paused(&self) -> bool {
        matches!(self.state, State::Paused(_))
    }

    /// The run is over and the title would be shown.
    #[cfg(test)]
    pub fn over(&self) -> bool {
        matches!(self.state, State::Over)
    }

    /// The driving HUD is shown (its task is killed before the report card).
    pub fn hud_visible(&self) -> bool {
        !matches!(
            self.state,
            State::Report { .. }
                | State::RankingPending(_)
                | State::Ranking { .. }
                | State::RunEnd(_)
                | State::Leaving(_)
                | State::Over
        )
    }

    /// The remaining time the HUD shows while the result has deducted but the rules have not.
    pub fn remaining_override(&self) -> Option<i32> {
        match &self.state {
            State::Result(s) => s.remaining_after(),
            State::AfterResult { remaining_s, .. } => *remaining_s,
            _ => None,
        }
    }

    /// The pause check at the top of the waiting and driving loops (0x4080d0): on the pause
    /// edge, opens the dialogs instead of this tick's simulation; the opening tick places the
    /// frame. Returns whether the tick was taken.
    pub fn try_pause(&mut self, input: &MenuInput, phase: DrivePhase, sfx: &mut Vec<Sfx>) -> bool {
        let driving = matches!(phase, DrivePhase::PreDeparture | DrivePhase::Driving);
        if !(input.pause && driving && matches!(self.state, State::Run { .. })) {
            return false;
        }
        let mut menu = PauseMenu::new(self.defs.pause.clone());
        menu.tick(&MenuInput::NONE, sfx);
        self.state = State::Paused(Box::new(menu));
        true
    }

    /// One frozen tick of the pause. The result comes on its last frozen tick.
    pub fn frozen_tick(&mut self, input: &MenuInput, sfx: &mut Vec<Sfx>) -> Option<PauseResult> {
        let State::Paused(menu) = &mut self.state else {
            return None;
        };
        let r = menu.tick(input, sfx)?;
        self.reset();
        Some(r)
    }

    /// After one simulation tick.
    pub fn after_tick(
        &mut self,
        obs: &Observation,
        input: &MenuInput,
        sfx: &mut Vec<Sfx>,
    ) -> Option<Action> {
        match &mut self.state {
            State::Run { arrived, game_over } => {
                match obs.phase {
                    DrivePhase::Arrived => *arrived += 1,
                    DrivePhase::GameOver => *game_over += 1,
                    _ => (*arrived, *game_over) = (0, 0),
                }
                if *arrived == RESULT_AFTER_ARRIVED_TICKS {
                    let s = StationResultScreen::new(self.defs.result.clone(), obs.result, sfx);
                    self.state = State::Result(Box::new(s));
                } else if *game_over >= CONTINUE_AFTER_GAME_OVER_TICKS && !obs.time_up_running {
                    self.state = if obs.can_continue {
                        State::Continue(Box::new(ContinueScreen::new(self.defs.continue_)))
                    } else {
                        State::EndWait {
                            ticks: REPORT_AFTER_NO_CONTINUE_OFFERED,
                            cleared: false,
                        }
                    };
                }
                None
            }
            State::Paused(_) => None,
            State::Result(s) => {
                if !s.tick(input, obs.remaining_s, sfx) {
                    self.state = State::AfterResult {
                        remaining_s: s.remaining_after(),
                        waited: false,
                    };
                }
                None
            }
            State::AfterResult { waited, .. } => {
                if *waited {
                    None
                } else {
                    *waited = true;
                    Some(Action::FinishStation)
                }
            }
            State::ContinueWait(n) => {
                *n -= 1;
                if *n == 0 {
                    self.state =
                        State::Continue(Box::new(ContinueScreen::new(self.defs.continue_)));
                }
                None
            }
            State::Continue(c) => match c.tick(input)? {
                true => {
                    self.reset();
                    Some(Action::ContinueRun)
                }
                false => {
                    self.state = State::EndWait {
                        ticks: REPORT_AFTER_CONTINUE_DECLINED,
                        cleared: false,
                    };
                    None
                }
            },
            State::EndWait { ticks, cleared } => {
                *ticks -= 1;
                if *ticks == 0 {
                    let card = ReportCard::new(self.defs.report.clone(), obs.grades);
                    self.state = State::Report {
                        card: Box::new(card),
                        cleared: *cleared,
                    };
                }
                None
            }
            State::Report { card, cleared } => {
                if card.tick(input, sfx) {
                    return None;
                }
                let cleared = *cleared;
                self.state = State::RankingPending(RunEndInput {
                    cleared,
                    stage: obs.stage,
                    stage_kind: obs.stage_kind,
                    grade: card.total(),
                    continues: obs.continues,
                });
                Some(Action::Ranking { cleared })
            }
            // The game did not answer `Action::Ranking`: no ranking.
            State::RankingPending(_) => {
                self.start_ranking(None, sfx);
                None
            }
            State::Ranking { entry, banner } => {
                if entry.tick(input, sfx) {
                    return None;
                }
                let done = Action::RankingDone {
                    table: entry.table(),
                    rows: *entry.rows(),
                    commit: entry.committed(),
                };
                let banner = RunEndBanner::new(self.defs.run_end, *banner, sfx);
                self.state = State::RunEnd(Box::new(banner));
                Some(done)
            }
            State::RunEnd(b) => {
                if !b.tick(input) {
                    self.state = State::Leaving(TITLE_AFTER_BANNER);
                }
                None
            }
            State::Leaving(n) => {
                *n -= 1;
                if *n == 0 {
                    self.state = State::Over;
                    return Some(Action::RunOver);
                }
                None
            }
            State::Over => None,
        }
    }

    /// The answer to [`Action::Ranking`] (0x41e8d0): the name entry when the route has a
    /// ranking and the value makes the top ten, else the run-end banner at once.
    pub fn start_ranking(&mut self, req: Option<&RankingRequest<'_>>, sfx: &mut Vec<Sfx>) {
        let State::RankingPending(banner) = self.state else {
            return;
        };
        let entry = req.and_then(|r| NameEntry::new(self.defs.ranking.clone(), r));
        self.state = match entry {
            Some(entry) => State::Ranking {
                entry: Box::new(entry),
                banner,
            },
            None => State::RunEnd(Box::new(RunEndBanner::new(self.defs.run_end, banner, sfx))),
        };
    }

    /// `Drive::finish_station` returned `outcome` (0x40d340).
    pub fn station_finished(&mut self, outcome: StationOutcome, can_continue: bool) {
        self.state = match outcome {
            StationOutcome::Continue => State::Run {
                arrived: 0,
                game_over: 0,
            },
            StationOutcome::Cleared => State::EndWait {
                ticks: REPORT_AFTER_LAST_STATION,
                cleared: true,
            },
            StationOutcome::GameOver if can_continue => {
                State::ContinueWait(CONTINUE_AFTER_STATION_GAME_OVER)
            }
            StationOutcome::GameOver => State::EndWait {
                ticks: REPORT_AFTER_LAST_STATION,
                cleared: false,
            },
        };
    }

    /// Clears the slots the last [`StageFlow::draw`] wrote (screens end or change slots).
    /// Call before the HUD writes its slots.
    pub fn clear_drawn(&mut self, layer: &mut SpriteLayer) {
        for i in self.drawn.drain(..) {
            if let Some(s) = layer.slots.get_mut(i) {
                *s = Slot::CLEAR;
            }
        }
    }

    /// Draws the active screen over the HUD.
    pub fn draw(&mut self, layer: &mut SpriteLayer) {
        let slots = match &self.state {
            State::Paused(m) => m.slots(),
            State::Result(s) => s.slots(),
            State::Continue(c) => c.slots(),
            State::Report { card, .. } => card.slots(),
            State::RunEnd(b) => b.slots(),
            State::Ranking { entry, .. } => {
                entry.draw(layer);
                self.drawn = entry.slots().indices().collect();
                return;
            }
            _ => return,
        };
        slots.draw(layer);
        self.drawn = slots.indices().collect();
    }
}

#[cfg(test)]
mod tests {
    use super::super::tables::{Dialog, TextDesc};
    use super::*;
    use crate::screens::pause::DialogDef;

    fn defs() -> FlowDefs {
        let dialog = |items: Vec<i8>| DialogDef {
            dialog: Dialog {
                frame: 0x541,
                title: 1,
                title_y: 16,
                cursor_x: 24,
                items_y: 64,
                spacing: 40,
                items,
            },
            frame_size: (320, 192),
        };
        FlowDefs {
            pause: PauseDefs {
                main: dialog(vec![3, 5, 7]),
                quit: dialog(vec![11, 13]),
                retry: dialog(vec![11, 13]),
                text: vec![TextDesc::default(); 29],
                screen: (640, 480),
            },
            result: ResultDefs {
                layout: Vec::new(),
                lesson_layout: Vec::new(),
                result_digits: [0; 10],
                time_digits: [0; 10],
                overrun_banners: [0; 4],
            },
            continue_: ContinueDefs {
                origin: (0, 0),
                prompt_pos: (304, 344),
                chosen_pos: (240, 200),
                time_digits: [0; 10],
            },
            report: ReportDefs {
                origin: (0, 0),
                layout: Vec::new(),
                result_digits: [0; 10],
            },
            run_end: RunEndDefs {
                origin: (0, 0),
                message_pos: (304, 344),
                girl_pos: (384, 48),
            },
            ranking: RankingDefs {
                t: crate::screens::ranking::tests::tables(),
                time_digits: [0; 10],
            },
        }
    }

    fn obs(phase: DrivePhase) -> Observation {
        Observation {
            phase,
            can_continue: true,
            remaining_s: 30,
            result: ResultInput {
                tolerance_m: 2,
                dist_m: 0.3,
                overrun_factor: 1,
                ..Default::default()
            },
            grades: [10; 5],
            continues: 0,
            stage: 1,
            stage_kind: 1,
            time_up_running: false,
        }
    }

    const NONE: MenuInput = MenuInput::NONE;

    /// Ticks `phase` until an action comes; returns (ticks, action).
    fn until_action(
        f: &mut StageFlow,
        o: &Observation,
        input: impl Fn(u32) -> MenuInput,
    ) -> (u32, Action) {
        let mut sfx = Vec::new();
        for n in 1..5000 {
            if let Some(a) = f.after_tick(o, &input(n), &mut sfx) {
                return (n, a);
            }
        }
        panic!("no action");
    }

    #[test]
    fn station_result_then_finish() {
        let mut f = StageFlow::new(defs());
        let (n, a) = until_action(&mut f, &obs(DrivePhase::Arrived), |_| NONE);
        // Created after 5 Arrived ticks, 207 result ticks, finish one tick later.
        assert_eq!((n, a), (5 + 207 + 1, Action::FinishStation));
        f.station_finished(StationOutcome::Continue, true);
        assert!(matches!(f.state, State::Run { arrived: 0, .. }));
    }

    #[test]
    fn time_out_offers_continue() {
        let mut f = StageFlow::new(defs());
        let (n, a) = until_action(&mut f, &obs(DrivePhase::GameOver), |n| MenuInput {
            confirm: n == 30,
            ..NONE
        });
        // Continue created after 22 ticks; confirm on its 8th tick, 30 more ticks.
        assert_eq!((n, a), (30 + 30, Action::ContinueRun));
    }

    #[test]
    fn declined_continue_runs_report_and_banner() {
        let mut f = StageFlow::new(defs());
        let o = obs(DrivePhase::GameOver);
        let (n, a) = until_action(&mut f, &o, |_| NONE);
        // 22 + 430 continue + 4 + 281 report.
        assert_eq!(
            (n, a),
            (22 + 430 + 4 + 281, Action::Ranking { cleared: false })
        );
        assert!(!f.hud_visible());
        f.start_ranking(None, &mut Vec::new());
        let (n, a) = until_action(&mut f, &o, |_| NONE);
        assert_eq!((n, a), (151 + 2, Action::RunOver));
        assert!(f.over());
    }

    #[test]
    fn name_entry_between_report_and_banner() {
        use crate::save::{ROWS, Row};
        let mut f = StageFlow::new(defs());
        f.station_finished(StationOutcome::Cleared, false);
        let o = obs(DrivePhase::Cleared);
        let (_, a) = until_action(&mut f, &o, |_| NONE);
        assert_eq!(a, Action::Ranking { cleared: true });
        let rankings = vec![[Row::EMPTY; ROWS]; 13];
        let req = RankingRequest {
            route: 1,
            score: 500,
            pro: true,
            power: 0,
            rankings: &rankings,
        };
        f.start_ranking(Some(&req), &mut Vec::new());
        // Time-out after 930 ticks, then the page's 151.
        let (n, a) = until_action(&mut f, &o, |_| NONE);
        let Action::RankingDone {
            table,
            rows,
            commit,
        } = a
        else {
            panic!("{a:?}");
        };
        assert_eq!((n, table, commit), (930 + 151, 3, false));
        assert_eq!((rows[0].score, rows[0].pro), (500, true));
        let (n, a) = until_action(&mut f, &o, |_| NONE);
        assert_eq!((n, a), (151 + 2, Action::RunOver));
    }

    #[test]
    fn no_continue_on_the_last_leg() {
        let mut f = StageFlow::new(defs());
        let o = Observation {
            can_continue: false,
            ..obs(DrivePhase::GameOver)
        };
        let (n, a) = until_action(&mut f, &o, |_| NONE);
        assert_eq!((n, a), (24 + 281, Action::Ranking { cleared: false }));
    }

    #[test]
    fn cleared_runs_report_after_three_ticks() {
        let mut f = StageFlow::new(defs());
        f.station_finished(StationOutcome::Cleared, false);
        let (n, a) = until_action(&mut f, &obs(DrivePhase::Cleared), |n| MenuInput {
            confirm: n == 3 + 31,
            ..NONE
        });
        // Report created on tick 3, first counting tick 34 is skipped to 225.
        assert_eq!((n, a), (34 + 25, Action::Ranking { cleared: true }));
    }

    #[test]
    fn station_game_over_continues_after_one_tick() {
        let mut f = StageFlow::new(defs());
        f.station_finished(StationOutcome::GameOver, true);
        let (n, _) = until_action(&mut f, &obs(DrivePhase::GameOver), |n| MenuInput {
            confirm: n == 2,
            ..NONE
        });
        assert_eq!(n, 2 + 30);
    }

    #[test]
    fn pause_only_while_driving() {
        let mut f = StageFlow::new(defs());
        let mut sfx = Vec::new();
        let esc = MenuInput {
            pause: true,
            cancel: true,
            ..NONE
        };
        assert!(!f.try_pause(&esc, DrivePhase::Arrived, &mut sfx));
        assert!(!f.try_pause(&NONE, DrivePhase::Driving, &mut sfx));
        assert!(f.try_pause(&esc, DrivePhase::Driving, &mut sfx));
        assert!(f.paused());
        assert_eq!(f.frozen_tick(&esc, &mut sfx), None);
        assert_eq!(f.frozen_tick(&NONE, &mut sfx), None);
        assert_eq!(f.frozen_tick(&NONE, &mut sfx), Some(PauseResult::Resume));
        assert!(!f.paused());
    }
}
