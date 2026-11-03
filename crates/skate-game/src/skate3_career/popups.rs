//! The career's screens: retail's own movies where [`super::screens::retail`] has one (37 Skate 101, 111 the milestone
//! pages, 23 the poster, 109 the reward summary), on the career layer; the rest on retail's `sk8popup` (the popup the
//! online menu shows) with the screen's text and options, over play. `Not retail (story mode stage 1 build)`: the
//! teammate page (112) is still the sk8popup stand-in (plan Task 19); the answers it sends are retail's.
//! Not tested in play.

use bevy::prelude::*;
use serde_json::Value as Json;

use super::glue::{Career, Shown};
use super::screens::{self as career_screens, Press};
use crate::apt_screen::{CAREER, Screens};
use crate::core_menu_runtime::{Key, fe_sound_id};
use crate::popup_runtime::{self, Runtime};

const FRAME: f32 = 1. / 60.;
/// Drawn over the online layer's own popup.
const DRAW_KEY: &str = "zz-career";
/// The career layer's screen name for a retail movie.
const RETAIL: &str = "career";
/// How long a closed retail movie may take over its outro before it goes.
const OUTRO_SECONDS: f32 = 1.;

#[derive(Resource, Default)]
pub struct Popups {
    /// The sk8popup manifest, read on first use; `Some(None)`: unavailable (warned once).
    source: Option<Option<Json>>,
    open: Option<Runtime>,
    accumulated: f32,
    /// Whether this pinned the player's input (others pin it too: set and cleared on the edges only).
    pinned: bool,
    /// A retail movie is up on the career layer, with this many A presses to go.
    retail: Option<usize>,
    /// Its keys wait for its intro to end.
    wait_intro: bool,
    /// Seconds a closed retail movie has played its outro.
    closing: Option<f32>,
}

/// Opens `shown`'s retail movie on the career layer: its A presses and whether keys wait for its intro, or None to
/// use the stand-in.
fn open_retail(career: &Career, shown: &Shown, screens: Option<&mut Screens>) -> Option<(usize, bool)> {
    let retail = career_screens::retail(career.state.as_ref()?, &shown.asking)?;
    let layer = screens?.layer(CAREER);
    let answers: Vec<(&str, serde_json::Value)> = retail.answers.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    let opened = layer.open(RETAIL, retail.movie, &answers).and_then(|screen| {
        if screen.has("OnLoaded") {
            screen.call("OnLoaded", &[])?;
        }
        screen.play("intro")?;
        retail.state.map_or(Ok(()), |n| screen.call("SetState", &[n.into()]).map(drop))
    });
    match opened {
        Ok(()) => Some((retail.pages, retail.wait_intro)),
        Err(e) => {
            layer.close(RETAIL);
            warn!("Skate 3 career screen {}: {e}; the stand-in popup shows it", retail.movie);
            None
        }
    }
}

/// An answer as the glue reads it: B on a cancelable popup is its last option (the poster's Decline).
fn answer(chosen: usize, options: usize) -> usize {
    if chosen == popup_runtime::CANCEL { options.saturating_sub(1) } else { chosen }
}

#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
fn run(
    mut career: ResMut<Career>,
    mut popups: ResMut<Popups>,
    config: Res<crate::config::Config>,
    (keys, nav): (Res<ButtonInput<KeyCode>>, Res<crate::customiser::Navigation>),
    mut screens: Option<ResMut<crate::apt_screen::Screens>>,
    mut sounds: MessageWriter<crate::skate_audio::FrontEndSoundRequest>,
    real: Res<Time<Real>>,
    mut input: ResMut<crate::input::ControllerInput>,
) {
    let popups = &mut *popups;
    let waiting = career.shown.clone().filter(|_| career.answer.is_none());
    let down: Vec<Key> = [
        (nav.pressed & 1 != 0, Key::Up),
        (nav.pressed & 2 != 0, Key::Down),
        (nav.pressed & 4 != 0, Key::Left),
        (nav.pressed & 8 != 0, Key::Right),
        (keys.just_pressed(KeyCode::Enter) || nav.pressed & 0x1000 != 0, Key::Accept),
        (keys.just_pressed(KeyCode::Backspace) || nav.pressed & 0x2000 != 0, Key::Back),
    ]
    .into_iter()
    .filter_map(|(pressed, key)| pressed.then_some(key))
    .collect();
    if let (Some(shown), None, None) = (&waiting, &popups.open, popups.retail) {
        if let Some((pages, wait_intro)) = open_retail(&career, shown, screens.as_deref_mut()) {
            (popups.retail, popups.wait_intro, popups.closing) = (Some(pages), wait_intro, None);
        }
    }
    if let (Some(pages), Some(shown)) = (popups.retail, waiting.as_ref()) {
        let movie = screens.as_mut().and_then(|s| s.layer(CAREER).get(RETAIL));
        if let Some(screen) = movie {
            let keys = if popups.wait_intro && !screen.bindings.intro_complete { &[][..] } else { &down[..] };
            for &key in keys {
                match career_screens::press(&shown.asking, key, pages) {
                    Some(Press::Page(sound)) => {
                        sounds.write(crate::skate_audio::FrontEndSoundRequest(fe_sound_id(sound)));
                        if let Err(e) = screen.input(key) {
                            error!("Skate 3 career screen: {e}");
                        }
                        popups.retail = Some(pages - 1);
                    }
                    Some(Press::Close(chosen, sound)) => {
                        sounds.write_batch(sound.map(|s| crate::skate_audio::FrontEndSoundRequest(fe_sound_id(s))));
                        if let Err(e) = screen.play("outro") {
                            error!("Skate 3 career screen: {e}");
                        }
                        career.answer = Some(chosen);
                        popups.retail = None;
                        popups.closing = Some(0.);
                    }
                    None => continue,
                }
                break;
            }
        } else {
            // The layer lost it: answer as the stand-in would, so the career never stalls.
            career.answer = Some(0);
            popups.retail = None;
        }
    } else if popups.retail.is_some() {
        // Nothing waits on it any more.
        popups.retail = None;
        popups.closing = Some(OUTRO_SECONDS);
    }
    if let Some(seconds) = popups.closing.as_mut() {
        *seconds += real.delta_secs();
        if let Some(layer) = screens.as_mut().map(|s| s.layer(CAREER)) {
            let ended = layer.get(RETAIL).is_none_or(|s| s.bindings.outro_complete);
            if ended || *seconds >= OUTRO_SECONDS {
                layer.close(RETAIL);
                popups.closing = None;
            }
        } else {
            popups.closing = None;
        }
    }
    match (&waiting, popups.open.is_some()) {
        (Some(shown), false) if popups.retail.is_none() && career.answer.is_none() => {
            let source = popups.source.get_or_insert_with(|| {
                let path = config.asset_root.join("private/online-ui/runtime/sk8popup.json");
                let read = std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()));
                read.map_err(|e| warn!("Skate 3 career screens unavailable ({}: {e}); they answer at once", path.display())).ok()
            });
            match source.as_ref().map(|s| Runtime::load(s, &shown.text, shown.options, shown.cancelable)) {
                Some(Ok(runtime)) => {
                    popups.open = Some(runtime);
                    popups.accumulated = 0.;
                }
                // Nothing to show it with: the first option, so the career never stalls.
                other => {
                    if let Some(Err(e)) = other {
                        error!("Skate 3 career screen: {e}");
                    }
                    career.answer = Some(0);
                }
            }
        }
        (None, true) => popups.open = None,
        _ => {}
    }
    let up = popups.open.is_some() || popups.retail.is_some();
    if up != popups.pinned {
        popups.pinned = up;
        input.pinned = up;
    }
    let mut draws = None;
    if let (Some(runtime), Some(shown)) = (popups.open.as_mut(), waiting.as_ref()) {
        let mut chosen = None;
        for &key in &down {
            let result = runtime.input(key);
            sounds.write_batch(runtime.bindings.sounds.drain(..).map(crate::skate_audio::FrontEndSoundRequest));
            match result {
                Ok(Some(c)) => {
                    chosen = Some(c);
                    break;
                }
                Ok(None) => {}
                Err(e) => {
                    error!("Skate 3 career screen: {e}");
                    chosen = Some(0);
                    break;
                }
            }
        }
        if let Some(c) = chosen {
            career.answer = Some(answer(c, shown.options.len()));
            popups.open = None;
            popups.pinned = false;
            input.pinned = false;
        } else {
            popups.accumulated = (popups.accumulated + real.delta_secs()).min(4. * FRAME);
            let mut result = Ok(());
            while popups.accumulated >= FRAME && result.is_ok() {
                popups.accumulated -= FRAME;
                result = runtime.advance();
            }
            draws = Some(result.and_then(|()| crate::apt_scene::draw(&runtime.bindings.movie, &runtime.vm, &runtime.bindings.shapes)));
        }
    }
    let Some(screens) = screens.as_mut() else { return };
    let layer = screens.layer(crate::apt_screen::ONLINE);
    match draws {
        Some(Ok(draws)) => {
            layer.extra.insert(DRAW_KEY, draws);
        }
        Some(Err(e)) => {
            error!("Skate 3 career screen stopped: {e}");
            popups.open = None;
            popups.pinned = false;
            input.pinned = false;
            career.answer = Some(0);
            layer.extra.remove(DRAW_KEY);
        }
        None => {
            layer.extra.remove(DRAW_KEY);
        }
    }
}

pub(super) fn build(app: &mut App) {
    app.init_resource::<Popups>().add_systems(
        Update,
        run.before(crate::apt_screen::render)
            .run_if(|career: Res<Career>, popups: Res<Popups>| career.shown.is_some() || career.answer.is_some() || popups.closing.is_some()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b_on_the_poster_is_decline() {
        assert_eq!(answer(popup_runtime::CANCEL, 2), 1);
        assert_eq!(answer(0, 2), 0);
        assert_eq!(answer(1, 2), 1);
    }
}
