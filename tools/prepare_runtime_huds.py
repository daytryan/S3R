"""Export the two HUDs used by the engine, as one automatic setup stage.

Only runtime manifests and referenced RGBA payloads enter the installation.
Extraction intermediates stay in setup's conversion workspace and are removed
by its normal successful-install cleanup. No audio or full-disc extraction.
"""
import argparse
from pathlib import Path
import tempfile

from prepare_hud import prepare as prepare_scoring
from extract_session_marker import prepare as prepare_marker
from install_prepared_hud import install


def prepare(game, assets, work):
    # Retail texture names plus the install/generation prefix can exceed legacy
    # Windows path limits. Only compact runtime files belong in the installation.
    with tempfile.TemporaryDirectory(prefix='sk8hud-') as temporary:
        return prepare_in_workspace(game, assets, Path(temporary))


def prepare_in_workspace(game, assets, work):
    from asset_pipeline.optional_content import CONTENT_ERRORS, note
    from install_prepared_hud import runtime_files
    scoring = work / 'scoring'
    marker = work / 'session-marker'
    files = {}
    for name, source, action, is_marker in (
        ('hud', scoring, lambda: prepare_scoring(game, scoring, assets/'private/stock/skater-collections.json'), False),
        ('session-marker', marker, lambda: prepare_marker(game, marker), True)):
        availability=assets/'private'/(name+'-availability.json')
        try:
            action()
            runtime_files(source, is_marker)
        except CONTENT_ERRORS as error:
            try:
                runtime_files(assets/'private'/name,is_marker)
                retained=True
            except CONTENT_ERRORS:
                retained=False
                # Broken artwork must not be loaded as if it were complete.
                (assets/'private'/name/('hud.json' if is_marker else 'runtime/trickdisplay.json')).unlink(missing_ok=True)
            note(availability,name,error,retained)
            continue
        files.update(install(assets,None if is_marker else source,source if is_marker else None,replace=True))
        availability.unlink(missing_ok=True)
    files.update(prepare_start_menu(game, assets, work / 'core-menu'))
    files.update(prepare_create_skater(game, assets, work / 'create-skater'))
    files.update(prepare_challenge_map(game, assets, work / 'challenge-map'))
    files.update(prepare_tip_loader(game, assets, work / 'tip-loader'))
    files.update(prepare_music_ui(game, assets, work / 'music-ui'))
    files.update(prepare_hud_meters(game, assets, work / 'hud-meters'))
    print(f'Original runtime HUDs ready: {len(files)} verified files', flush=True)
    return files


def prepare_start_menu(game, assets, work):
    """Start-button crossbar (main/core_menu). Optional like the HUDs: on failure a
    verified previous copy is kept, otherwise the game uses its classic pause panel."""
    from asset_pipeline.optional_content import CONTENT_ERRORS, note
    import prepare_core_menu as menu
    availability = assets/'private'/'core-menu-availability.json'
    try:
        menu.prepare(game, work, assets/'private/stock/skater-collections.json')
        installed = menu.install(assets, work)
    except CONTENT_ERRORS as error:
        try:
            menu.runtime_files(assets/'private'/'core-menu')
            retained = True
        except CONTENT_ERRORS:
            retained = False
            (assets/'private/core-menu'/menu.MANIFEST).unlink(missing_ok=True)
        note(availability, 'core-menu', error, retained)
        return {}
    availability.unlink(missing_ok=True)
    return {f'private/core-menu/{relative}': sha for relative, sha in installed.items()}


def prepare_create_skater(game, assets, work):
    """Original Edit Skater screen (cas/createskater); else the classic customiser panel."""
    import prepare_create_skater as screen
    return prepare_screen(screen, game, assets, work)


def prepare_challenge_map(game, assets, work):
    """Original Challenge Map screen (map/challengemap); else Challenge Map stays a Start menu picker."""
    import prepare_challenge_map as screen
    return prepare_screen(screen, game, assets, work)


def prepare_tip_loader(game, assets, work):
    """Original loading screen (main/tiploader); else map loads show the pause panel's progress."""
    import prepare_tip_loader as screen
    return prepare_screen(screen, game, assets, work)


def prepare_music_ui(game, assets, work):
    """Original now-playing chyron, Audio Settings and Music Player screens; else music plays without them."""
    import prepare_music_ui as screen
    return prepare_screen(screen, game, assets, work)


def prepare_hud_meters(game, assets, work):
    """Original Trick Analyzer and Manual Meter (Learn > On Screen Help); else they stay off."""
    import prepare_hud_meters as screen
    return prepare_screen(screen, game, assets, work)


def prepare_screen(screen, game, assets, work):
    """One original screen module. Optional: on failure a verified previous copy is kept,
    otherwise the game uses its engine fallback."""
    from asset_pipeline.optional_content import CONTENT_ERRORS, note
    availability = assets/'private'/f'{screen.FOLDER}-availability.json'
    try:
        screen.prepare(game, work, assets/'private/stock/skater-collections.json')
        installed = screen.install(assets, work)
    except CONTENT_ERRORS as error:
        try:
            screen.runtime_files(assets/'private'/screen.FOLDER)
            retained = True
        except CONTENT_ERRORS:
            retained = False
            (assets/'private'/screen.FOLDER/screen.MANIFEST).unlink(missing_ok=True)
        note(availability, screen.FOLDER, error, retained)
        return {}
    availability.unlink(missing_ok=True)
    return {f'private/{screen.FOLDER}/{relative}': sha for relative, sha in installed.items()}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--game', type=Path, required=True)
    parser.add_argument('--assets', type=Path, required=True)
    parser.add_argument('--work', type=Path, required=True)
    args = parser.parse_args()
    prepare(args.game, args.assets, args.work)
