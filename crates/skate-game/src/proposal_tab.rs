//! Not retail (user 10-13): the open proposal as a small tab in the top-left corner instead of retail's
//! full-screen SignUp_Poster, so play goes on. It shows the proposed row's location photo, title, mode and place,
//! description, proposer and seconds left; D-pad Left accepts, Right declines. Draws in the online layer's
//! 1280x720 stage with sk8popup's Futura and the movie button glyphs. Pure.
use crate::{
    apt_scene::{Draw, Vertex},
    apt_text::Font,
    challenge_map::Card,
    graphic_board::{button, colour, label, quad},
};

const WHITE: &str = "runtime/white.rgba";
const X: f32 = 24.;
const Y: f32 = 24.;
const W: f32 = 360.;
const PAD: f32 = 8.;
/// The retail popup's cyan, the poster's white and the legend's steel blue.
const CYAN: u32 = 0x6ED0FF;
const INK: u32 = 0xEBF2F8;
const STEEL: u32 = 0x96AFC3;
const DESCRIPTION_LINES: usize = 3;
/// The whole tab, scaled about its corner (user 10-13: "a little big").
const SCALE: f32 = 0.75;

/// A solid triangle pointing left (`-1.`) or right (`1.`), centred on `at`.
fn arrow(dir: f32, [x, y]: [f32; 2], size: f32, rgb: u32) -> Draw {
    let points = [[x + dir * size, y], [x - dir * size * 0.8, y - size], [x - dir * size * 0.8, y + size]];
    Draw { texture: WHITE.into(), size: [1, 1], vertices: points.map(|p| Vertex { position: p, uv: [0.5, 0.5] }).to_vec(), multiply: colour(rgb, 1.), add: [0.; 4] }
}

/// The tab for `card`, proposed by `by` with `left` seconds to answer.
pub(crate) fn draws(font: &Font, card: &Card, by: &str, left: f32, header: &str, words: [&str; 3]) -> Vec<Draw> {
    let [accept, decline, proposed_by] = words;
    let inner = W - 2. * PAD;
    let mut out = vec![quad(WHITE, [1, 1], [0.; 4], [0.; 4])];
    let mut y = Y + PAD;
    out.extend(label(font, header, [X + PAD + 2., y], 16., colour(CYAN, 1.)));
    let seconds = format!("{}", left.ceil().max(0.) as u32);
    out.extend(label(font, &seconds, [X + W - PAD - 2. - font.width(&seconds, 16.), y], 16., colour(INK, 1.)));
    y += 24.;
    if let Some((path, size)) = &card.photo {
        let h = inner * size[1] as f32 / size[0] as f32;
        // ponytail: square map pictures fill a 2:1 slot from their middle.
        let (h, v) = if h > inner / 2. { (inner / 2., (1. - inner / 2. / h) / 2.) } else { (h, 0.) };
        out.push(crate::graphic_board::quad_uv(path, *size, [X + PAD, y, X + PAD + inner, y + h], [0., v, 1., 1. - v], [1.; 4]));
        y += h + 8.;
    }
    out.extend(label(font, &card.title, [X + PAD + 2., y], 20., colour(INK, 1.)));
    y += 25.;
    let mode = [card.mode.as_str(), card.place.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("  -  ");
    out.extend(label(font, &mode, [X + PAD + 2., y], 15., colour(CYAN, 1.)));
    y += 21.;
    for line in font.lines(&card.description, 14., Some(inner - 4.)).into_iter().filter(|l| !l.is_empty()).take(DESCRIPTION_LINES) {
        out.extend(label(font, &line, [X + PAD + 2., y], 14., colour(STEEL, 1.)));
        y += 18.;
    }
    y += 6.;
    out.extend(label(font, &format!("{proposed_by} {by}"), [X + PAD + 2., y], 14., colour(STEEL, 1.)));
    // D-pad glyph, then <- Accept  -> Decline, right-aligned.
    let (ha, hd) = (font.width(accept, 15.), font.width(decline, 15.));
    let mut x = X + W - PAD - 2. - (22. + 14. + ha + 14. + 14. + hd);
    out.push(button("DPad", [x, y - 1.], 16.5));
    x += 26.;
    out.push(arrow(-1., [x + 4., y + 8.], 5., INK));
    out.extend(label(font, accept, [x + 12., y], 15., colour(INK, 1.)));
    x += 12. + ha + 14.;
    out.push(arrow(1., [x + 4., y + 8.], 5., INK));
    out.extend(label(font, decline, [x + 12., y], 15., colour(INK, 1.)));
    y += 20. + PAD;
    out[0] = quad(WHITE, [1, 1], [X, Y, X + W, y], [0.024, 0.04, 0.055, 0.85]);
    for v in out.iter_mut().flat_map(|d| d.vertices.iter_mut()) {
        v.position = [X + (v.position[0] - X) * SCALE, Y + (v.position[1] - Y) * SCALE];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tab as the host sees it, from the installed sk8popup font and challenge map. SKATE3_ASSET_ROOT = the
    /// install's assets; SKATE_ONLINE_UI_SHOTS saves proposal-tab.png.
    #[test]
    #[ignore = "needs INST\\assets"]
    fn the_tab_shows_the_row_photo_and_answers() {
        let assets = std::path::PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").expect("SKATE3_ASSET_ROOT"));
        let root = assets.join("private/online-ui");
        let source: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/sk8popup.json")).unwrap()).unwrap();
        let text = crate::apt_text::TextAssets::load(&source).unwrap();
        let font = text.fonts[&text.font_named("Futura Std Medium").unwrap()].clone();
        let card = Card {
            title: "Arbor Ledge".into(),
            mode: "Spot Battle".into(),
            place: "Downtown".into(),
            description: "This landmark tree spot houses some foliage-friendly ledges and gaps.".into(),
            photo: Some((assets.join("private/challenge-map/images/assets/data/fe/source/images/locations/uptown_promenade/0000_uptown_promenade.Texture.rgba").to_string_lossy().into_owned(), [512, 256])),
        };
        let draws = draws(&font, &card, "Ann", 26.4, "Challenge Proposed!", ["Accept", "Decline", "Proposed by:"]);
        let bottom = draws[0].vertices.iter().map(|v| v.position[1]).fold(0., f32::max);
        assert!(bottom < 300., "a corner tab, not a poster (bottom {bottom})");
        assert!(draws.iter().any(|d| d.texture.ends_with("uptown_promenade.Texture.rgba")), "photo drawn");
        if let Ok(out) = std::env::var("SKATE_ONLINE_UI_SHOTS") {
            crate::core_menu_runtime::tests::raster_draws(&draws, &root, &std::path::Path::new(&out).join("proposal-tab.png"));
        }
    }
}
