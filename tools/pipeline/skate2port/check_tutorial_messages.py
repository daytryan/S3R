"""Checks a skate2-tutorial export's progression messages (Task 2): the vault id and the named properties retail's
SendMessage reads (0x824d3020: type 1 branches on the id, type 4 on FileID/CharacterID, type 5 on TitleHALID/TextHALID),
and CameraMan 2727, the line type 4 SponsorMeStart plays.
usage: py -3 check_tutorial_messages.py SKATE2_TUTORIAL_DIR"""
import json, sys
from pathlib import Path

root = Path(sys.argv[1])
m = json.loads((root/'tutorial.json').read_text(encoding='utf-8'))['progression']['messages']
assert m['Show Unlocks Screen']['id'] == 10, m['Show Unlocks Screen']
assert m['SponsorMeStart']['id'] == 207 and m['SponsorMeStart']['named'] == {'FileID': 2727, 'CharacterID': 40}, m['SponsorMeStart']
assert m['201_Follow_Slappy']['id'] == 376, m['201_Follow_Slappy']
assert m['201_Follow_Slappy']['named'] == {'TextHALID': 'ID_201_TEXT_FOLLOW_SLAPPY', 'TitleHALID': 'ID_201_TITLE_FOLLOW_SLAPPY'}
assert m['Atiba']['named'] == {'Location Name': 'm_1a_01_postchallenge_prosarrivespawn_atiba', 'CharacterID': 5}, m['Atiba']
assert (root/'speech/CameraMan_2727.wav').is_file(), 'CameraMan 2727 not exported'
# What the disc lacks (searched 10-13): Pumping's intro scene, Atiba 1008/1015 and CameraMan 936. Anything else missing
# is an export regression: every other scene and line the seven lessons name is on the disc.
missing = {k: sorted(v) for k, v in json.loads((root/'tutorial.json').read_text(encoding='utf-8'))['missing'].items()}
assert missing == {'locators': [], 'volumes': [], 'strings': [], 'scenes': ['tutorial_m_2a_01_tutorial_slappy'],
                   'speech': ['Atiba 1008', 'Atiba 1015', 'CameraMan 936']}, missing
print('ok')
