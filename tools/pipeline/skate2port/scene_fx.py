"""Mixes a scene's ICE AUDIO_EFFECT keys (track 30: the photo_killed shutter clicks) into one wav from t=0, so the
engine plays it with the scene's other streams (event 5 / start_reel play every scene.audio entry at once).

usage: py scene_fx.py KEYS.json CLIPS_DIR TUTORIAL_DIR
  KEYS.json     {scene: [[seconds, effect id], ...]}; CLIPS_DIR/<id>.wav holds each effect (16-bit PCM)
  TUTORIAL_DIR  private/skate2-tutorial: writes nis/<scene>_fx.wav and adds it to tutorial.json's scene audio
"""
import array, json, sys, wave
from pathlib import Path


def read(path):
    with wave.open(str(path)) as w:
        assert w.getsampwidth() == 2, f'{path}: 16-bit PCM only'
        pcm = array.array('h', w.readframes(w.getnframes()))
        channels, rate = w.getnchannels(), w.getframerate()
    if channels == 1:
        pcm = array.array('h', (s for s in pcm for _ in range(2)))
    elif channels != 2:
        # ponytail: first two channels only (clicks are mono or stereo on the disc so far).
        pcm = array.array('h', (s for i, s in enumerate(pcm) if i % channels < 2))
    return pcm, rate


def mix(keys, clips_dir, out_wav, max_seconds):
    """Each [seconds, id] clip placed at its time over silence, summed (clipped to 16 bits), stereo.
    Returns the written length in seconds; nothing past `max_seconds` (it would hold the scene)."""
    clips = {i: read(Path(clips_dir) / f'{i}.wav') for _, i in keys}
    rates = {rate for _, rate in clips.values()}
    assert len(rates) == 1, f'clips at several rates {rates}'
    rate = rates.pop()
    end = max(int(t * rate) * 2 + len(clips[i][0]) for t, i in keys)
    end = min(end, int(max_seconds * rate) * 2)
    out = [0] * end
    for t, i in keys:
        at = int(t * rate) * 2
        for j, s in enumerate(clips[i][0][:max(0, end - at)]):
            out[at + j] += s
    out = array.array('h', (max(-32768, min(32767, s)) for s in out))
    Path(out_wav).parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(out_wav), 'wb') as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(out.tobytes())
    return len(out) / 2 / rate


def main(keys_json, clips_dir, tutorial_dir):
    tutorial_dir = Path(tutorial_dir)
    data = json.loads((tutorial_dir / 'tutorial.json').read_text(encoding='utf-8'))
    for scene, keys in json.loads(Path(keys_json).read_text()).items():
        entry = data['scenes'][scene]
        name = f'nis/{scene}_fx.wav'
        seconds = mix(keys, clips_dir, tutorial_dir / name, entry['duration'])
        entry['audio'] = [a for a in entry['audio'] if a['audio'] != name] + [{'audio': name, 'seconds': seconds}]
        print(f'{scene}: {len(keys)} effect(s) -> {name} ({seconds:.2f}s)')
    (tutorial_dir / 'tutorial.json').write_text(json.dumps(data, indent=1), encoding='utf-8')


def demo():
    import tempfile
    with tempfile.TemporaryDirectory() as d:
        d = Path(d)
        with wave.open(str(d / '1.wav'), 'wb') as w:
            w.setnchannels(1), w.setsampwidth(2), w.setframerate(100)
            w.writeframes(array.array('h', [1000, 2000]).tobytes())
        seconds = mix([[0, 1], [0.5, 1], [0.5, 1]], d, d / 'out.wav', 0.6)
        pcm, rate = read(d / 'out.wav')
        assert rate == 100 and seconds == 0.52, seconds
        assert pcm[:4].tolist() == [1000, 1000, 2000, 2000]          # t=0, mono to stereo
        assert pcm[100:104].tolist() == [2000, 2000, 4000, 4000]     # two keys at 0.5 s sum
        assert mix([[0.59, 1]], d, d / 'cut.wav', 0.6) == 0.6        # held to the scene's length
    print('scene_fx demo ok')


if __name__ == '__main__':
    demo() if len(sys.argv) == 1 else main(*sys.argv[1:])
