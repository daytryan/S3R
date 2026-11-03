import json
T = {int(k): v for k, v in json.load(open('text.json', encoding='utf-8')).items()}
def key(label):
    x = 0xFFFFFFFF
    for c in label.encode('latin-1'):
        x = (x * 33 + c) & 0xFFFFFFFF
    return x
def s(label):
    return T.get(key(label)) if isinstance(label, str) else None
