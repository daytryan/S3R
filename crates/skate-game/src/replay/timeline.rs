//! The editor's native timeline (`mCustomRender`, research Q1): replay_editor3 leaves an 817x30 box at (228.5, 53)
//! that the game draws into: the clip markers and the current channel's key markers, each glowing while the
//! playhead is over it (EXE 0x82509298: marker_default / marker_glow / key_default / key_glow). The playhead and the
//! in / out marks are the movie's own clips.
use super::editor::{Editor, MARKER_RADIUS};
use crate::apt_scene::{Draw, Vertex};

/// research Q1 [disc]: `mCustomRender` at (228.5, 53), 817x30.
pub(crate) const ORIGIN: [f32; 2] = [228.5, 53.];
pub(crate) const SIZE: [f32; 2] = [817., 30.];
pub(crate) const TEXTURES: [&str; 4] =
    ["textures/marker_default.png", "textures/marker_glow.png", "textures/key_default.png", "textures/key_glow.png"];

/// The x of normalised time `t` on the stage.
pub(crate) fn x(t: f32) -> f32 {
    ORIGIN[0] + t.clamp(0., 1.) * SIZE[0]
}

fn quad(texture: &str, size: [u32; 2], centre: f32) -> Draw {
    let [w, h] = size.map(|v| v as f32);
    let (x0, y0) = (centre - w * 0.5, ORIGIN[1] + (SIZE[1] - h) * 0.5);
    let points = [[x0, y0], [x0 + w, y0], [x0 + w, y0 + h], [x0, y0 + h]];
    let uvs = [[0., 0.], [1., 0.], [1., 1.], [0., 1.]];
    Draw {
        texture: texture.into(),
        size,
        vertices: [0, 1, 2, 0, 2, 3].map(|i| Vertex { position: points[i], uv: uvs[i] }).to_vec(),
        multiply: [1.; 4],
        add: [0.; 4],
    }
}

/// The markers to draw; `sizes` are the four textures' pixel sizes, in [`TEXTURES`] order.
pub(crate) fn draws(editor: &Editor, sizes: [[u32; 2]; 4]) -> Vec<Draw> {
    let under = |t: f32| (t - editor.playhead).abs() <= MARKER_RADIUS;
    let keys = editor.keys[editor.channel.index()].iter().filter(|k| k.soft).map(|k| (k.t, 2));
    let markers = editor.clip_markers.iter().map(|&t| (t, 0));
    keys.chain(markers)
        .map(|(t, texture)| {
            let i = texture + usize::from(under(t));
            quad(TEXTURES[i], sizes[i], x(t))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_sit_on_the_817_px_bar_and_glow_under_the_playhead() {
        let mut editor = Editor::new(false);
        editor.state = super::super::editor::State::MainEdit;
        editor.playhead = 0.25;
        editor.drop_marker().unwrap();
        editor.playhead = 0.75;
        editor.drop_marker().unwrap();
        let draws = draws(&editor, [[16, 30]; 4]);
        assert_eq!(draws.len(), 2);
        let centre = |d: &Draw| (d.vertices[0].position[0] + d.vertices[1].position[0]) * 0.5;
        assert!((centre(&draws[0]) - (228.5 + 0.25 * 817.)).abs() < 1e-3);
        assert_eq!(draws[0].texture, TEXTURES[0]);
        assert_eq!(draws[1].texture, TEXTURES[1], "the marker under the playhead glows");
    }
}
