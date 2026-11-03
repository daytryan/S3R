//! Not retail (user 10-11, item 31): Create a Skater's colour wheel, the clothing, grip and hair colour rows'
//! (cas_menu::Custom). It is the movie's own page: the page's card and title with the colour row alone on it (its
//! swatch shows the colour as it changes), the info panel's help, the movie's legend (B Cancel, A Accept, Y Reset).
//! Only the hue / saturation disc and the brightness bar are new, on the card's dark panel under the row, their
//! cursors the movie's own swatch and highlight. The skater wears the colour as it changes; A keeps it, B puts back
//! what the row had, Y goes to the item's own colour.
use crate::{
    apt_scene::Draw,
    cas_menu::Custom,
    create_skater_runtime::{Page, Row},
    customiser::Customiser,
    customiser_parts::Parts,
};
use bevy::prelude::{ButtonInput, KeyCode, Resource, Vec2};
use skate_core::input::xbox::XboxState;
use serde_json::Value;
use std::f32::consts::TAU;

/// On the movie's 1280x720 stage: the card's dark panel under the colour row (graphic_board::CAS_CARD; the info
/// panel starts at 472), the disc (centre, radius) and the brightness bar on it.
const ROOM: [f32; 4] = [96., 200., 444., 468.];
const CENTRE: Vec2 = Vec2::new(237., 331.);
const RADIUS: f32 = 112.;
const BAR: [f32; 4] = [393., 219., 415., 443.];
/// Where the movie draws the colour row's swatch and its highlight (the wheel page's only row): the cursors' art.
const SWATCH_BOX: [f32; 4] = [94., 134., 154., 196.];
/// The cursors' size across, the frames' unselected-row blue (graphic_board's DIM), and the fade in.
const CURSOR: f32 = 26.;
const DIM: [f32; 3] = [0.235, 0.353, 0.471];
const FADE: f32 = 0.15;
/// The wheel page's info panel.
pub(crate) const HELP: &str = "#Move the left stick to pick the color. Right stick up and down sets how bright it is. Y resets it to the item's own color.";
/// A full stick crosses this much of the disc's radius a second, a held brightness control this much of the bar.
/// ponytail: feel, untested on a pad; tune here.
const STICK_SPEED: f32 = 1.2;
const BRIGHTNESS_SPEED: f32 = 0.8;
/// The pictures the screen adds for the wheel (create_skater: once, as it first opens).
pub(crate) const DISC_TEXTURE: &str = "colour_wheel/disc";
pub(crate) const BAR_TEXTURE: &str = "colour_wheel/bar";
pub(crate) const DISC_SIZE: u32 = 256;
pub(crate) const BAR_SIZE: [u32; 2] = [1, 256];

/// A colour as the wheel holds it: hue (0-1 round the disc), saturation (out from the centre), brightness.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Wheel {
    pub hue: f32,
    pub saturation: f32,
    pub value: f32,
}
fn hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h6 = h.rem_euclid(1.) * 6.;
    let f = |n: f32| {
        let k = (n + h6) % 6.;
        v - v * s * k.min(4. - k).clamp(0., 1.)
    };
    [f(5.), f(3.), f(1.)]
}
impl Wheel {
    pub fn of([r, g, b]: [f32; 3]) -> Self {
        let max = r.max(g).max(b);
        let d = max - r.min(g).min(b);
        let hue = (if d <= 0. {
            0.
        } else if max == r {
            ((g - b) / d).rem_euclid(6.)
        } else if max == g {
            (b - r) / d + 2.
        } else {
            (r - g) / d + 4.
        }) / 6.;
        Self { hue, saturation: if max > 0. { d / max } else { 0. }, value: max }
    }
    pub fn rgb(self) -> [f32; 3] {
        hsv(self.hue, self.saturation, self.value)
    }
    /// The cursor on the disc (unit radius, y up).
    pub fn point(self) -> Vec2 {
        Vec2::from_angle(self.hue * TAU) * self.saturation
    }
    /// Put the cursor at `p` (kept on the disc; the centre keeps the hue).
    pub fn set_point(&mut self, p: Vec2) {
        let p = p.clamp_length_max(1.);
        self.saturation = p.length();
        if self.saturation > 1e-4 {
            self.hue = (p.y.atan2(p.x) / TAU).rem_euclid(1.);
        }
    }
}

/// The open wheel (create_skater's). LT / RT still turn the skater (customiser::rotate_preview, user 10-13).
#[derive(Resource, Default)]
pub(crate) struct ColourWheel(pub Option<Open>);
/// The controls this frame: the cursor's stick (unit, y up) and brightness (-1 to 1). Pad: left stick (XInput's
/// dead zone), right stick up / down or D-pad Left / Right; keyboard: arrows / WASD, Q / E.
pub(crate) fn controls(pad: Option<&XboxState>, keys: &ButtonInput<KeyCode>) -> (Vec2, f32) {
    let (buttons, stick, right) = pad.map_or((0, [0; 2], [0; 2]), |s| (s.buttons, s.left, s.right));
    let key = |plus: [KeyCode; 2], minus: [KeyCode; 2]| f32::from(u8::from(keys.any_pressed(plus))) - f32::from(u8::from(keys.any_pressed(minus)));
    let axis = |v: i16| Some((f32::from(v) / 32767.).clamp(-1., 1.)).filter(|v| v.abs() >= 0.24).unwrap_or(0.);
    let held = |bit: u16| f32::from(u8::from(buttons & bit != 0));
    let steer = Vec2::new(
        axis(stick[0]) + key([KeyCode::ArrowRight, KeyCode::KeyD], [KeyCode::ArrowLeft, KeyCode::KeyA]),
        axis(stick[1]) + key([KeyCode::ArrowUp, KeyCode::KeyW], [KeyCode::ArrowDown, KeyCode::KeyS]),
    );
    (steer, axis(right[1]) + held(8) - held(4) + key([KeyCode::KeyE; 2], [KeyCode::KeyQ; 2]))
}
/// The open wheel.
pub(crate) struct Open {
    /// The look before it opened: B puts it back.
    pub before: Value,
    pub custom: Custom,
    pub wheel: Wheel,
    /// The colour the skater wears from the wheel: only a change is applied (A at once keeps the look as it was).
    last: Wheel,
    /// Steering waits for every control to come back first: the press that landed on the swatch is still held.
    settled: bool,
    /// What the mouse drags since its press.
    drag: Option<Drag>,
    /// Seconds open (the fade in).
    shown: f32,
}
#[derive(Clone, Copy, PartialEq)]
enum Drag {
    Disc,
    Bar,
}
impl Open {
    /// Open on a colour row: the wheel on the colour worn.
    pub fn new(c: &Customiser, custom: Custom) -> Self {
        let wheel = Wheel::of(custom.start);
        Self { before: c.draft.clone(), wheel, last: wheel, custom, settled: false, drag: None, shown: 0. }
    }
    /// Hold to move: `stick` (unit, y up) moves the cursor and `brightness` (-1 to 1) the bar, over `dt` seconds.
    pub fn steer(&mut self, stick: Vec2, brightness: f32, dt: f32) {
        self.shown += dt;
        if !self.settled {
            self.settled = stick == Vec2::ZERO && brightness == 0.;
            return;
        }
        if stick != Vec2::ZERO {
            self.wheel.set_point(self.wheel.point() + stick.clamp_length_max(1.) * STICK_SPEED * dt);
        }
        self.wheel.value = (self.wheel.value + brightness.clamp(-1., 1.) * BRIGHTNESS_SPEED * dt).clamp(0., 1.);
    }
    /// The left mouse button at `at` (stage units): a press on the disc or the bar picks there and drags it.
    pub fn mouse(&mut self, at: Vec2, press: bool, held: bool) {
        if press {
            let inside = |[x0, y0, x1, y1]: [f32; 4]| (x0 - 8.0..=x1 + 8.0).contains(&at.x) && (y0 - 8.0..=y1 + 8.0).contains(&at.y);
            self.drag = if at.distance(CENTRE) <= RADIUS + 8. {
                Some(Drag::Disc)
            } else if inside(BAR) {
                Some(Drag::Bar)
            } else {
                None
            };
        } else if !held {
            self.drag = None;
        }
        match self.drag {
            Some(Drag::Disc) => self.wheel.set_point(Vec2::new(at.x - CENTRE.x, CENTRE.y - at.y) / RADIUS),
            Some(Drag::Bar) => self.wheel.value = ((BAR[3] - at.y) / (BAR[3] - BAR[1])).clamp(0., 1.),
            None => {}
        }
    }
    /// The skater in the wheel's colour, once it moved; true when it did.
    pub fn preview(&mut self, c: &mut Customiser, parts: &Parts) -> bool {
        let moved = self.wheel != self.last;
        if moved {
            self.last = self.wheel;
            c.apply(parts, Some(&self.custom.patch(self.wheel.rgb())));
        }
        moved
    }
    /// Y: the item's own colour (no grip colour on the grip), the wheel on it.
    pub fn default(&mut self, c: &mut Customiser, parts: &Parts) {
        c.apply(parts, Some(&self.custom.default));
        self.wheel = Wheel::of(self.custom.authored);
        self.last = self.wheel;
    }
    /// B: the look as it was before the wheel opened.
    pub fn cancel(self, c: &mut Customiser) {
        c.draft = self.before;
        c.redraw = true;
    }
    /// The disc and the bar on the card under the colour row, framed in the movie's row blue, their cursors the
    /// movie's swatch and highlight (from its `movie` draws this frame), fading in as the page does.
    pub fn draws(&self, movie: &[Draw]) -> Vec<Draw> {
        use crate::boot::{WHITE, quad};
        let fade = (self.shown / FADE).clamp(0., 1.);
        let grow = |[x0, y0, x1, y1]: [f32; 4], d: f32| [x0 - d, y0 - d, x1 + d, y1 + d];
        let full = hsv(self.wheel.hue, self.wheel.saturation, 1.);
        let disc = [CENTRE.x - RADIUS, CENTRE.y - RADIUS, CENTRE.x + RADIUS, CENTRE.y + RADIUS];
        // The disc's frame: its own picture's shape in one colour.
        let mut ring = quad(DISC_TEXTURE, [DISC_SIZE; 2], grow(disc, 2.), [0., 0., 0., fade]);
        ring.add = [DIM[0], DIM[1], DIM[2], 0.];
        let mut out = vec![
            ring,
            quad(DISC_TEXTURE, [DISC_SIZE; 2], disc, [1., 1., 1., fade]),
            quad(WHITE, [1, 1], grow(BAR, 2.), [DIM[0], DIM[1], DIM[2], fade]),
            quad(BAR_TEXTURE, BAR_SIZE, BAR, [full[0], full[1], full[2], fade]),
        ];
        let point = CENTRE + self.wheel.point() * Vec2::new(RADIUS, -RADIUS);
        let level = Vec2::new((BAR[0] + BAR[2]) / 2., BAR[3] - self.wheel.value * (BAR[3] - BAR[1]));
        for at in [point, level] {
            out.extend(frame_at(movie, at, CURSOR).into_iter().map(|mut d| {
                d.multiply[3] *= fade;
                d
            }));
        }
        out
    }
}
/// The wheel's page: `page` (the edit page) with its colour row `row` alone on it, the swatch in `rgb`, the info
/// panel's help, and A Accept / B Cancel.
pub(crate) fn page_for(page: &Page, row: usize, rgb: [f32; 3]) -> Page {
    let mut colour = page.rows.get(row).cloned().unwrap_or_default();
    colour.colours = vec![crate::cas_menu::SWATCH(rgb)];
    colour.value = 0.;
    colour.description = HELP.into();
    Page { rows: vec![colour], index: 0, accept: "ID_COMMON_ACCEPT".into(), back: "ID_COMMON_CANCEL".into(), ..page.clone() }
}
/// The movie's colour row swatch and its highlight (what it draws in SWATCH_BOX), `size` across, centred on `at`.
fn frame_at(movie: &[Draw], at: Vec2, size: f32) -> Vec<Draw> {
    use crate::graphic_board::bounds;
    let inside = |b: [f32; 4]| b[0] >= SWATCH_BOX[0] && b[1] >= SWATCH_BOX[1] && b[2] <= SWATCH_BOX[2] && b[3] <= SWATCH_BOX[3];
    let art: Vec<&Draw> = movie.iter().filter(|d| bounds(d).is_some_and(inside)).collect();
    let Some(all) = art.iter().filter_map(|d| bounds(d)).reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]) else {
        // ponytail: no swatch drawn (the page still coming in): a plain white mark.
        return vec![crate::boot::quad(crate::boot::WHITE, [1, 1], [at.x - 3., at.y - 3., at.x + 3., at.y + 3.], [1.; 4])];
    };
    let centre = Vec2::new((all[0] + all[2]) / 2., (all[1] + all[3]) / 2.);
    let scale = size / (all[2] - all[0]).max(all[3] - all[1]).max(1.);
    art.into_iter()
        .cloned()
        .map(|mut d| {
            d.vertices.iter_mut().for_each(|v| v.position = (at + (Vec2::from(v.position) - centre) * scale).into());
            d
        })
        .collect()
}
/// The disc's picture: hue round it, saturation out from the centre, full brightness, clear outside.
pub(crate) fn disc_rgba() -> Vec<u8> {
    let n = DISC_SIZE as f32;
    let mut out = Vec::with_capacity((DISC_SIZE * DISC_SIZE * 4) as usize);
    for y in 0..DISC_SIZE {
        for x in 0..DISC_SIZE {
            let p = Vec2::new((x as f32 + 0.5) / n * 2. - 1., 1. - (y as f32 + 0.5) / n * 2.);
            let mut w = Wheel { hue: 0., saturation: 0., value: 1. };
            w.set_point(p);
            let alpha = ((1. - p.length()) * n * 0.5 + 0.5).clamp(0., 1.);
            out.extend(w.rgb().map(|v| (v * 255.).round() as u8));
            out.push((alpha * 255.).round() as u8);
        }
    }
    out
}
/// The bar's picture: white at the top to black at the bottom (the hue's full colour multiplies it).
pub(crate) fn bar_rgba() -> Vec<u8> {
    (0..BAR_SIZE[1]).flat_map(|y| {
        let v = 255 - (y * 255 / (BAR_SIZE[1] - 1)) as u8;
        [v, v, v, 255]
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom() -> Custom {
        let default = serde_json::json!({"hair_tint": null});
        Custom { template: serde_json::json!({"hair_tint": [0, 0, 0]}), at: "/hair_tint".into(), start: [0.5; 3], authored: [0.5; 3], default, hair: false }
    }

    /// The user (10-11): getting to the wheel scrolled off screen. The disc and the bar sit on the card's dark panel
    /// under the colour row, on the movie's 1280x720 stage, which the canvas fits whole in any window (AutoMin,
    /// graphic_creator::to_movie): 16:9 and 21:9 alike.
    #[test]
    fn the_wheel_is_on_the_card_and_on_screen_at_16_9_and_21_9() {
        let inside = |outer: [f32; 4], inner: [f32; 4]| outer[0] <= inner[0] && outer[1] <= inner[1] && inner[2] <= outer[2] && inner[3] <= outer[3];
        let card = crate::graphic_board::CAS_CARD;
        for window in [Vec2::new(1920., 1080.), Vec2::new(2560., 1080.), Vec2::new(3440., 1440.), Vec2::new(1280., 1024.)] {
            let (a, b) = (crate::graphic_creator::to_movie(Vec2::ZERO, window), crate::graphic_creator::to_movie(window, window));
            assert!(inside([a.x, a.y, b.x, b.y], card), "{window}: the window shows stage {a}..{b}");
        }
        let disc = [CENTRE.x - RADIUS, CENTRE.y - RADIUS, CENTRE.x + RADIUS, CENTRE.y + RADIUS];
        for part in [BAR, disc] {
            assert!(inside(ROOM, [part[0] - 2., part[1] - 2., part[2] + 2., part[3] + 2.]), "{part:?} framed on the card's panel");
        }
        assert!(inside(card, ROOM) && ROOM[1] >= SWATCH_BOX[3], "under the colour row");
    }

    #[test]
    fn the_wheel_is_the_movies_own_page_with_the_colour_row_alone_on_it() {
        use crate::create_skater_runtime::Mode;
        let row = Row { text: "ID_CAC_WEARSTYLES_COLOUR".into(), kind: "color", colours: vec![1], description: "x".into(), ..Default::default() };
        let page = Page {
            title: "ID_CAC_WEAR_STYLES_TITLE".into(),
            mode: Mode::Morph,
            rows: vec![Row::default(), row],
            index: 0,
            accept: "ID_COMMON_ACCEPT".into(),
            back: "ID_COMMON_BACK".into(),
        };
        let wheel = page_for(&page, 1, [1., 0., 0.]);
        assert_eq!((wheel.title.as_str(), wheel.mode, wheel.rows.len(), wheel.index), ("ID_CAC_WEAR_STYLES_TITLE", Mode::Morph, 1, 0));
        assert_eq!((wheel.rows[0].text.as_str(), wheel.rows[0].kind, wheel.rows[0].colours.as_slice()), ("ID_CAC_WEARSTYLES_COLOUR", "color", &[0xff0000][..]));
        assert_eq!(wheel.rows[0].description, HELP);
        assert_eq!((wheel.accept.as_str(), wheel.back.as_str()), ("ID_COMMON_ACCEPT", "ID_COMMON_CANCEL"));
    }

    #[test]
    fn the_cursors_are_the_movies_swatch_and_highlight_moved_and_scaled() {
        use crate::boot::quad;
        let movie = vec![
            quad("swatch", [1, 1], [104., 146., 140., 182.], [1.; 4]),
            quad("glow", [1, 1], [98., 140., 146., 188.], [1.; 4]),
            quad("title", [1, 1], [96., 60., 300., 100.], [1.; 4]),
        ];
        let out = frame_at(&movie, Vec2::new(200., 300.), 24.);
        assert_eq!(out.iter().map(|d| d.texture.as_str()).collect::<Vec<_>>(), ["swatch", "glow"]);
        let b = crate::graphic_board::bounds(&out[1]).unwrap();
        assert!(b.iter().zip([188., 288., 212., 312.]).all(|(a, e)| (a - e).abs() < 1e-3), "{b:?}");
        let mut parts = Parts::default();
        parts.library.defaults.insert("male".into(), serde_json::json!({}));
        let mut open = Open::new(&Customiser::for_test(&parts, Vec::new(), "male"), custom());
        open.steer(Vec2::ZERO, 0., 1.);
        let drawn = open.draws(&movie);
        assert_eq!(drawn.iter().filter(|d| d.texture == "glow").count(), 2, "the disc's cursor and the bar's");
    }

    #[test]
    fn the_right_stick_and_the_d_pad_set_the_brightness_and_the_left_stick_the_colour() {
        let keys = ButtonInput::<KeyCode>::default();
        let pad = |triggers: [u8; 2], buttons: u16, left: [i16; 2], right: [i16; 2]| XboxState { buttons, triggers, left, right };
        // LT / RT turn the skater while the wheel is open (user 10-13), so they leave the brightness alone.
        assert_eq!(controls(Some(&pad([255, 255], 0, [0; 2], [0; 2])), &keys), (Vec2::ZERO, 0.), "LT / RT");
        assert_eq!(controls(Some(&pad([0; 2], 0, [0; 2], [0, 32767])), &keys).1, 1., "right stick up");
        assert_eq!(controls(Some(&pad([0; 2], 0, [0; 2], [0, -32767])), &keys).1, -1., "right stick down");
        assert_eq!(controls(Some(&pad([0; 2], 8, [0; 2], [0; 2])), &keys).1, 1., "D-pad Right");
        assert_eq!(controls(Some(&pad([0; 2], 0, [32767, 2000], [0; 2])), &keys).0, Vec2::X, "left stick, past the dead zone");
        let mut keys = keys;
        keys.press(KeyCode::KeyQ);
        keys.press(KeyCode::KeyW);
        assert_eq!(controls(None, &keys), (Vec2::Y, -1.));
    }

    #[test]
    fn colours_go_round_the_wheel_and_back() {
        for rgb in [[1., 0., 0.], [0.25, 0.5, 0.75], [0.1, 0.1, 0.1], [0., 0., 0.], [1., 1., 1.], [0.9, 0.8, 0.05]] {
            let back = Wheel::of(rgb).rgb();
            assert!(rgb.iter().zip(back).all(|(a, b)| (a - b).abs() < 1e-5), "{rgb:?} -> {back:?}");
        }
        let red = Wheel::of([1., 0., 0.]);
        assert!((red.point() - Vec2::X).length() < 1e-5, "red is right on the rim");
    }

    #[test]
    fn the_stick_moves_the_cursor_while_held_and_waits_for_the_landing_press_to_let_go() {
        let custom = custom();
        let mut open = Open { before: Value::Null, wheel: Wheel::of(custom.start), last: Wheel::of(custom.start), custom, settled: false, drag: None, shown: 0. };
        let start = open.wheel.point();
        open.steer(Vec2::X, 1., 0.1);
        assert_eq!(open.wheel.point(), start, "the press that landed on Custom is still held");
        open.steer(Vec2::ZERO, 0., 0.1);
        open.steer(Vec2::X, 0., 0.1);
        let moved = open.wheel.point() - start;
        assert!((moved.x - STICK_SPEED * 0.1).abs() < 1e-4 && moved.y.abs() < 1e-4, "held, not snapped: {moved}");
        for _ in 0..100 {
            open.steer(Vec2::X, 0., 0.1);
        }
        assert!((open.wheel.point().length() - 1.).abs() < 1e-4, "kept on the disc");
        let before = open.wheel.value;
        open.steer(Vec2::ZERO, -1., 0.25);
        assert!((open.wheel.value - (before - BRIGHTNESS_SPEED * 0.25)).abs() < 1e-4);
        for _ in 0..10 {
            open.steer(Vec2::ZERO, -1., 0.25);
        }
        assert_eq!(open.wheel.value, 0.);
    }

    #[test]
    fn the_mouse_picks_and_drags_on_the_disc_and_the_bar() {
        let custom = custom();
        let mut open = Open { before: Value::Null, wheel: Wheel::of(custom.start), last: Wheel::of(custom.start), custom, settled: true, drag: None, shown: 0. };
        open.mouse(CENTRE + Vec2::new(RADIUS, 0.), true, true);
        assert!(open.wheel.rgb().iter().zip([0.5, 0., 0.]).all(|(a, b)| (a - b).abs() < 1e-4), "{:?}", open.wheel.rgb());
        // Dragged off the disc, it keeps to the rim.
        open.mouse(CENTRE + Vec2::new(0., -3. * RADIUS), false, true);
        assert!((open.wheel.point() - Vec2::Y).length() < 1e-4);
        open.mouse(Vec2::new(BAR[0] + 5., BAR[1]), false, false);
        open.mouse(Vec2::new(BAR[0] + 5., BAR[1]), true, true);
        assert_eq!(open.wheel.value, 1.);
        open.mouse(Vec2::new(BAR[0] + 5., BAR[3] + 50.), false, true);
        assert_eq!(open.wheel.value, 0.);
        open.mouse(Vec2::ZERO, true, true);
        open.mouse(CENTRE, false, true);
        assert_eq!(open.wheel.value, 0., "a press outside picks nothing");
    }

    #[test]
    fn the_pictures_are_their_sizes() {
        assert_eq!(disc_rgba().len(), (DISC_SIZE * DISC_SIZE * 4) as usize);
        assert_eq!(bar_rgba().len(), (BAR_SIZE[0] * BAR_SIZE[1] * 4) as usize);
        assert_eq!(&bar_rgba()[..4], &[255, 255, 255, 255]);
        assert_eq!(disc_rgba()[3], 0, "clear outside the disc");
    }
}
