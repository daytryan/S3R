//! Drive player one's pad from a timeline file, in the retail recomp's `skate3_input_script`
//! format, so a session replays the exact inputs the retail audio captures were recorded with.
//!
//! `SKATE_INPUT_SCRIPT=<file>` enables it. One step per line: `<ms> [a b x y lb rb l3 r3 start
//! back up down left right] [lx= ly= rx= ry= lt= rt=]`, sticks -100..100 (up/right positive),
//! triggers 0..100; `@mark NAME` writes a marker into the audio trace; `#` comments. Like retail it
//! starts `SKATE_INPUT_SCRIPT_SETTLE_MS` (default 3000) after gameplay is first polled; the other
//! pads read as disconnected. With `SKATE_INPUT_SCRIPT_EXIT=1` the game exits 2 s after the end.
use super::platform::{DeviceError, DevicePacket};
use skate_core::input::xbox::XboxState;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const NEUTRAL: XboxState = XboxState { buttons: 0, triggers: [0; 2], left: [0; 2], right: [0; 2] };

struct Script {
    /// (end of step, pad) in timeline order.
    steps: Vec<(Duration, XboxState)>,
    marks: Vec<(Duration, String)>,
    settle: Duration,
    start: Option<Instant>,
    next_mark: usize,
    packet: u32,
    exit: bool,
}

static SCRIPT: Mutex<Option<Option<Script>>> = Mutex::new(None);

fn parse(text: &str) -> Result<(Vec<(Duration, XboxState)>, Vec<(Duration, String)>), String> {
    let (mut steps, mut marks, mut at) = (Vec::new(), Vec::new(), Duration::ZERO);
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if let Some(name) = line.strip_prefix("@mark") {
            marks.push((at, name.trim().to_owned()));
            continue;
        }
        if line.is_empty() || line.starts_with('@') {
            continue;
        }
        let mut tokens = line.split_whitespace();
        let ms: u64 = tokens.next().and_then(|t| t.parse().ok()).ok_or(format!("bad step '{line}'"))?;
        let mut pad = NEUTRAL;
        for token in tokens {
            if let Some((key, value)) = token.split_once('=') {
                let value: i32 = value.parse().map_err(|_| format!("bad value '{token}'"))?;
                let axis = (value.clamp(-100, 100) * 32767 / 100) as i16;
                let trigger = (value.clamp(0, 100) * 255 / 100) as u8;
                match key {
                    "lx" => pad.left[0] = axis,
                    "ly" => pad.left[1] = axis,
                    "rx" => pad.right[0] = axis,
                    "ry" => pad.right[1] = axis,
                    "lt" => pad.triggers[0] = trigger,
                    "rt" => pad.triggers[1] = trigger,
                    _ => return Err(format!("unknown axis '{token}'")),
                }
            } else {
                pad.buttons |= match token {
                    "up" => 0x0001, "down" => 0x0002, "left" => 0x0004, "right" => 0x0008,
                    "start" => 0x0010, "back" => 0x0020, "l3" => 0x0040, "r3" => 0x0080,
                    "lb" => 0x0100, "rb" => 0x0200, "a" => 0x1000, "b" => 0x2000,
                    "x" => 0x4000, "y" => 0x8000,
                    _ => return Err(format!("unknown button '{token}'")),
                };
            }
        }
        at += Duration::from_millis(ms);
        steps.push((at, pad));
    }
    Ok((steps, marks))
}

fn load() -> Option<Script> {
    let path = std::env::var_os("SKATE_INPUT_SCRIPT")?;
    let (steps, marks) = std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|text| parse(&text))
        .map_err(|e| eprintln!("SKATE_INPUT_SCRIPT {}: {e}", path.to_string_lossy()))
        .ok()?;
    let settle = std::env::var("SKATE_INPUT_SCRIPT_SETTLE_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(3000);
    Some(Script {
        steps,
        marks,
        settle: Duration::from_millis(settle),
        start: None,
        next_mark: 0,
        packet: 0,
        exit: std::env::var_os("SKATE_INPUT_SCRIPT_EXIT").is_some(),
    })
}

/// The scripted pad for slot `index`, or `None` when no script is loaded.
pub(super) fn pad(index: usize) -> Option<Result<DevicePacket, DeviceError>> {
    let mut guard = SCRIPT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let script = guard.get_or_insert_with(load).as_mut()?;
    if index != 0 {
        return Some(Err(DeviceError::Disconnected));
    }
    let now = Instant::now();
    let start = *script.start.get_or_insert(now + script.settle);
    let t = now.saturating_duration_since(start);
    while now >= start && script.next_mark < script.marks.len() && script.marks[script.next_mark].0 <= t {
        crate::skate_audio::player::trace::mark(&script.marks[script.next_mark].1);
        script.next_mark += 1;
    }
    let end = script.steps.last().map_or(Duration::ZERO, |s| s.0);
    if script.exit && now >= start && t > end + Duration::from_secs(2) {
        crate::skate_audio::player::trace::mark("script_exit");
        crate::skate_audio::player::trace::flush();
        std::process::exit(0);
    }
    let state = if now < start {
        NEUTRAL
    } else {
        script.steps.iter().find(|s| t < s.0).map_or(NEUTRAL, |s| {
            let p = &s.1;
            XboxState { buttons: p.buttons, triggers: p.triggers, left: p.left, right: p.right }
        })
    };
    script.packet = script.packet.wrapping_add(1);
    Some(Ok(DevicePacket { number: script.packet, state, subtype: 1 }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_the_retail_script_format() {
        let (steps, marks) = super::parse("# c\n@mark push\n150 a\n450\n@capture x\n250 ry=-100 lt=100\n").unwrap();
        assert_eq!(marks, vec![(std::time::Duration::ZERO, "push".to_owned())]);
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0].1.buttons, 0x1000);
        assert_eq!(steps[2].0.as_millis(), 850);
        assert_eq!((steps[2].1.right[1], steps[2].1.triggers[0]), (-32767, 255));
    }
}
