//! Scripted input, for checking behaviour without a person at the keyboard.
//!
//! `--script "mode; move 640 400; click; wait 30"` plays the steps back one
//! per frame through the same handlers real devices use. Together with
//! `--smoke --screenshot` this lets an automated run type into a window or
//! open a menu and then look at the result.
//!
//! Steps, separated by `;`:
//!
//! | Step              | Effect                                              |
//! | ----------------- | --------------------------------------------------- |
//! | `wait N`          | Let N frames pass                                   |
//! | `mode`            | Switch between walk and interact mode               |
//! | `use`             | Walk mode: use the window under the crosshair       |
//! | `turn YAW PITCH`  | Walk mode: turn left and up by degrees              |
//! | `walk RIGHT FWD`  | Walk mode: move by metres                           |
//! | `move X Y`        | Put the pointer at X,Y on the output (logical px)   |
//! | `press`/`release` | Left mouse button down or up                        |
//! | `click`           | Left button down, then up on the next frame         |
//! | `scroll PIXELS`   | Scroll vertically; positive is down                 |
//! | `key CODE`        | Tap a key, by Linux evdev code (30 is A, 28 Enter)  |

use std::collections::VecDeque;

use glam::Vec2;
use smithay::{
    backend::input::{Axis, AxisSource, ButtonState, InputTime, KeyState},
    input::{keyboard::Keycode, pointer::AxisFrame},
};

use crate::{input::BTN_LEFT, state::State};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Step {
    Wait(u32),
    Mode,
    Use,
    Turn { yaw: f32, pitch: f32 },
    Walk(Vec2),
    Move(Vec2),
    Press,
    Release,
    Click,
    Scroll(f64),
    Key(u32),
}

#[derive(Debug)]
pub struct Script {
    steps: VecDeque<Step>,
    /// Frames still to let pass before the next step.
    wait: u32,
    /// Hold off the first step until a client window has been drawn.
    wait_for_window: bool,
}

impl Script {
    pub fn parse(text: &str, wait_for_window: bool) -> Result<Self, String> {
        let steps = text
            .split(';')
            .map(str::trim)
            .filter(|step| !step.is_empty())
            .map(parse_step)
            .collect::<Result<_, _>>()?;
        Ok(Self {
            steps,
            wait: 0,
            wait_for_window,
        })
    }

    pub fn is_finished(&self) -> bool {
        self.steps.is_empty() && self.wait == 0
    }

    /// The step to run this frame, if any.
    fn next(&mut self, window_shown: bool) -> Option<Step> {
        if self.wait_for_window && !window_shown {
            return None;
        }
        self.wait_for_window = false;
        if self.wait > 0 {
            self.wait -= 1;
            return None;
        }
        self.steps.pop_front()
    }
}

fn parse_step(step: &str) -> Result<Step, String> {
    let mut words = step.split_whitespace();
    let name = words.next().unwrap_or_default();
    let arguments: Vec<&str> = words.collect();

    let number = |index: usize| -> Result<f64, String> {
        arguments
            .get(index)
            .and_then(|word| word.parse().ok())
            .ok_or_else(|| {
                format!(
                    "script step `{step}`: expected a number as argument {}",
                    index + 1
                )
            })
    };
    let expect = |count: usize| {
        if arguments.len() == count {
            Ok(())
        } else {
            Err(format!(
                "script step `{step}`: expected {count} argument(s)"
            ))
        }
    };

    let parsed = match name {
        "wait" => {
            expect(1)?;
            Step::Wait(number(0)? as u32)
        }
        "mode" => {
            expect(0)?;
            Step::Mode
        }
        "use" => {
            expect(0)?;
            Step::Use
        }
        "turn" => {
            expect(2)?;
            Step::Turn {
                yaw: (number(0)? as f32).to_radians(),
                pitch: (number(1)? as f32).to_radians(),
            }
        }
        "walk" => {
            expect(2)?;
            Step::Walk(Vec2::new(number(0)? as f32, number(1)? as f32))
        }
        "move" => {
            expect(2)?;
            Step::Move(Vec2::new(number(0)? as f32, number(1)? as f32))
        }
        "press" => {
            expect(0)?;
            Step::Press
        }
        "release" => {
            expect(0)?;
            Step::Release
        }
        "click" => {
            expect(0)?;
            Step::Click
        }
        "scroll" => {
            expect(1)?;
            Step::Scroll(number(0)?)
        }
        "key" => {
            expect(1)?;
            Step::Key(number(0)? as u32)
        }
        _ => return Err(format!("unknown script step `{step}`")),
    };
    Ok(parsed)
}

impl State {
    /// Play the next step of the script, if there is one. Call once a frame.
    pub fn run_script_step(&mut self) {
        let window_shown = self.windows.iter().any(|entry| entry.texture.is_some());
        let Some(step) = self
            .script
            .as_mut()
            .and_then(|script| script.next(window_shown))
        else {
            return;
        };
        tracing::debug!(?step, "script");

        let now = InputTime::now();
        match step {
            Step::Wait(frames) => {
                if let Some(script) = self.script.as_mut() {
                    script.wait = frames;
                }
            }
            Step::Mode => self.toggle_mode(),
            Step::Use => self.use_hovered(),
            Step::Turn { yaw, pitch } => self.camera.turn(yaw, pitch),
            Step::Walk(step) => self.camera.walk(step),
            Step::Move(position) => self.pointer_moved_to(position, now),
            Step::Press => self.pointer_button(BTN_LEFT, ButtonState::Pressed, now),
            Step::Release => self.pointer_button(BTN_LEFT, ButtonState::Released, now),
            Step::Click => {
                self.pointer_button(BTN_LEFT, ButtonState::Pressed, now);
                if let Some(script) = self.script.as_mut() {
                    script.steps.push_front(Step::Release);
                }
            }
            Step::Scroll(pixels) => {
                let frame = AxisFrame::new(now)
                    .source(AxisSource::Wheel)
                    .value(Axis::Vertical, pixels);
                self.pointer_axis(frame);
            }
            Step::Key(evdev_code) => {
                // xkb key codes are evdev codes offset by 8.
                let keycode = Keycode::new(evdev_code + 8);
                self.key(keycode, KeyState::Pressed, now);
                self.key(keycode, KeyState::Released, now);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_sequence_of_steps() {
        let script = Script::parse("mode; move 640 400.5 ;click;; wait 3; key 30", false).unwrap();
        assert_eq!(
            Vec::from(script.steps),
            [
                Step::Mode,
                Step::Move(Vec2::new(640.0, 400.5)),
                Step::Click,
                Step::Wait(3),
                Step::Key(30),
            ]
        );
    }

    #[test]
    fn turn_takes_degrees() {
        let script = Script::parse("turn 90 -45", false).unwrap();
        let Step::Turn { yaw, pitch } = script.steps[0] else {
            panic!("not a turn");
        };
        assert!((yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!((pitch + std::f32::consts::FRAC_PI_4).abs() < 1e-6);
    }

    #[test]
    fn rejects_bad_steps() {
        assert!(Script::parse("jump", false).is_err());
        assert!(Script::parse("move 1", false).is_err());
        assert!(Script::parse("move 1 two", false).is_err());
        assert!(Script::parse("click 3", false).is_err());
    }

    #[test]
    fn steps_come_one_per_frame_and_wait_holds_them_back() {
        let mut script = Script::parse("mode; use", false).unwrap();
        assert_eq!(script.next(false), Some(Step::Mode));
        // A `wait 2` sets this, as `run_script_step` does.
        script.wait = 2;
        assert!(!script.is_finished());
        assert_eq!(script.next(false), None);
        assert_eq!(script.next(false), None);
        assert_eq!(script.next(false), Some(Step::Use));
        assert!(script.is_finished());
        assert_eq!(script.next(false), None);
    }

    #[test]
    fn can_hold_off_until_a_window_is_shown() {
        let mut script = Script::parse("mode", true).unwrap();
        assert_eq!(script.next(false), None);
        assert!(!script.is_finished());
        assert_eq!(script.next(true), Some(Step::Mode));
    }
}
