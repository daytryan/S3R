//! User 10-14 (feature #12, also bug #9): modern flat controller prompts in place of retail's 360-era glossy ones,
//! drawn by tools/make_button_icons.py. Each keeps its retail texture's size and placement, so every layout holds.
//! A PlayStation pad gets its own set (`ps/`, [`PS_PREFIX`]): cross, circle, square, triangle, L1..R2, Create, Options.
//! ponytail: Skate 2's direction variants (DPad_Up, Stick_Left_L, ...) and Skate 1's prompts keep retail's art.

/// The texture path a PlayStation glyph is drawn from (input::pad_glyphs): `ps-buttons/<Xbox name>.rgba`.
pub(crate) const PS_PREFIX: &str = "ps-buttons/";
/// The PlayStation set's names: the Xbox button in each one's place.
pub(crate) const PS_NAMES: [&str; 15] = [
    "A", "B", "Back", "DPad", "L_Bumper", "L_Trigger", "R_Bumper", "R_Trigger", "Start", "Stick_Left", "Stick_Right", "X", "Y",
    "L3_Click", "R3_Click",
];
/// Glyphs Skate 3's disc set lacks (bug 10-17 #30/#39): the replay editor's hide prompt, the Trick Guide's Hall of Meat
/// prompts and the reset-object tips ask for them; apt_scene::glyph serves our art to every movie. Bug 10-25 #21: the
/// Skate 2 Gestures rows' D-pad directions; retail's one-arm `DPad_Up` .. names reach them too (apt_scene::lit_arm).
/// Voice chat's mics (user 10-26): a talker's (MicOn) and a silent one's (MicOff), for the name tags, the roster grid,
/// the Player List's rows and our own.
pub(crate) const EXTRA_GLYPHS: [&str; 8] = ["L3_Click", "R3_Click", "DPadUp", "DPadDown", "DPadLeft", "DPadRight", "MicOn", "MicOff"];

/// The new art for a retail button texture `path` of `size`, if it is one of the replaced buttons.
pub(crate) fn replace(path: &str, size: [u32; 2]) -> Option<&'static [u8]> {
    let file = path.rsplit(['/', '\\']).next()?;
    if let Some(name) = path.strip_prefix(PS_PREFIX).and_then(|p| p.strip_suffix(".rgba")).filter(|_| size == [40, 30]) {
        return ps(name);
    }
    let (name, game) = if let Some(n) = file.strip_suffix(".rgba").filter(|_| path.contains("runtime/buttons/") && size == [40, 30]) {
        (n, 3)
    } else if let Some(n) = file.strip_suffix(".Texture.rgba").filter(|_| path.contains("buttons/xbox360/buttons/") && size == [64, 64]) {
        (n.split_once("_button_")?.1, 2)
    } else {
        return None;
    };
    Some(if game == 3 { s3(name)? } else { s2(name)? })
}

fn s3(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "A" => include_bytes!("button_icons/s3/A.rgba"),
        "B" => include_bytes!("button_icons/s3/B.rgba"),
        "Back" => include_bytes!("button_icons/s3/Back.rgba"),
        "DPad" => include_bytes!("button_icons/s3/DPad.rgba"),
        "L_Bumper" => include_bytes!("button_icons/s3/L_Bumper.rgba"),
        "L_Trigger" => include_bytes!("button_icons/s3/L_Trigger.rgba"),
        "R_Bumper" => include_bytes!("button_icons/s3/R_Bumper.rgba"),
        "R_Trigger" => include_bytes!("button_icons/s3/R_Trigger.rgba"),
        "Start" => include_bytes!("button_icons/s3/Start.rgba"),
        "Stick_Left" => include_bytes!("button_icons/s3/Stick_Left.rgba"),
        "Stick_Right" => include_bytes!("button_icons/s3/Stick_Right.rgba"),
        "X" => include_bytes!("button_icons/s3/X.rgba"),
        "Y" => include_bytes!("button_icons/s3/Y.rgba"),
        "L3_Click" => include_bytes!("button_icons/s3/L3_Click.rgba"),
        "R3_Click" => include_bytes!("button_icons/s3/R3_Click.rgba"),
        "DPadUp" => include_bytes!("button_icons/s3/DPadUp.rgba"),
        "DPadDown" => include_bytes!("button_icons/s3/DPadDown.rgba"),
        "DPadLeft" => include_bytes!("button_icons/s3/DPadLeft.rgba"),
        "DPadRight" => include_bytes!("button_icons/s3/DPadRight.rgba"),
        "MicOn" => include_bytes!("button_icons/s3/MicOn.rgba"),
        "MicOff" => include_bytes!("button_icons/s3/MicOff.rgba"),
        _ => return None,
    })
}

fn ps(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "A" => include_bytes!("button_icons/ps/A.rgba"),
        "B" => include_bytes!("button_icons/ps/B.rgba"),
        "Back" => include_bytes!("button_icons/ps/Back.rgba"),
        "DPad" => include_bytes!("button_icons/ps/DPad.rgba"),
        "L_Bumper" => include_bytes!("button_icons/ps/L_Bumper.rgba"),
        "L_Trigger" => include_bytes!("button_icons/ps/L_Trigger.rgba"),
        "R_Bumper" => include_bytes!("button_icons/ps/R_Bumper.rgba"),
        "R_Trigger" => include_bytes!("button_icons/ps/R_Trigger.rgba"),
        "Start" => include_bytes!("button_icons/ps/Start.rgba"),
        "Stick_Left" => include_bytes!("button_icons/ps/Stick_Left.rgba"),
        "Stick_Right" => include_bytes!("button_icons/ps/Stick_Right.rgba"),
        "X" => include_bytes!("button_icons/ps/X.rgba"),
        "Y" => include_bytes!("button_icons/ps/Y.rgba"),
        "L3_Click" => include_bytes!("button_icons/ps/L3_Click.rgba"),
        "R3_Click" => include_bytes!("button_icons/ps/R3_Click.rgba"),
        _ => return None,
    })
}

fn s2(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "A" => include_bytes!("button_icons/s2/A.rgba"),
        "B" => include_bytes!("button_icons/s2/B.rgba"),
        "Back" => include_bytes!("button_icons/s2/Back.rgba"),
        "DPad" => include_bytes!("button_icons/s2/DPad.rgba"),
        "L_Bumper" => include_bytes!("button_icons/s2/L_Bumper.rgba"),
        "L_Trigger" => include_bytes!("button_icons/s2/L_Trigger.rgba"),
        "R_Bumper" => include_bytes!("button_icons/s2/R_Bumper.rgba"),
        "R_Trigger" => include_bytes!("button_icons/s2/R_Trigger.rgba"),
        "Start" => include_bytes!("button_icons/s2/Start.rgba"),
        "Stick_Left" => include_bytes!("button_icons/s2/Stick_Left.rgba"),
        "Stick_Right" => include_bytes!("button_icons/s2/Stick_Right.rgba"),
        "X" => include_bytes!("button_icons/s2/X.rgba"),
        "Y" => include_bytes!("button_icons/s2/Y.rgba"),
        "L3_Click" => include_bytes!("button_icons/s2/L3_Click.rgba"),
        "R3_Click" => include_bytes!("button_icons/s2/R3_Click.rgba"),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::replace;

    #[test]
    fn retail_buttons_get_the_new_art_at_their_own_size() {
        assert_eq!(replace("runtime/buttons/A.rgba", [40, 30]).map(<[u8]>::len), Some(40 * 30 * 4));
        let s2 = "assets/data/fe/source/images/buttons/xbox360/buttons/0016_button_Start.Texture.rgba";
        assert_eq!(replace(s2, [64, 64]).map(<[u8]>::len), Some(64 * 64 * 4));
        assert!(replace("assets/data/fe/source/images/buttons/xbox360/buttons/0008_button_DPad_Up.Texture.rgba", [64, 64]).is_none());
        assert!(replace("runtime/buttons/A.rgba", [64, 64]).is_none(), "another size is another texture");
        for name in super::PS_NAMES {
            let art = replace(&format!("{}{name}.rgba", super::PS_PREFIX), [40, 30]);
            assert_eq!(art.map(<[u8]>::len), Some(40 * 30 * 4), "{name}");
            // The D-pad cross and the three-line Menu / Options icon are the same on both pads.
            if !["DPad", "Start"].contains(&name) {
                assert_ne!(art, replace(&format!("runtime/buttons/{name}.rgba"), [40, 30]), "{name}: its own art");
            }
        }
        for name in super::EXTRA_GLYPHS {
            assert_eq!(replace(&format!("runtime/buttons/{name}.rgba"), [40, 30]).map(<[u8]>::len), Some(40 * 30 * 4), "{name}");
        }
        // Full circles: the face buttons' art stays inside the texture (bug #9's clipped tops).
        let a = replace("runtime/buttons/A.rgba", [40, 30]).unwrap();
        assert!((0..40).all(|x| a[x * 4 + 3] == 0 && a[(29 * 40 + x) * 4 + 3] == 0), "top and bottom rows clear");
    }
}
