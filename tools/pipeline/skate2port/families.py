"""Python mirror of create_skater.rs wear_families(): report families mixing Skate 2 (s2_) and Skate 3 models."""


def families(catalog):
    result = {}
    for component in catalog['components']:
        models = component['models']
        ids = lambda m: [v['id'] for g in m['lods'][0]['material_instances'] for v in g] if m['lods'] else []
        tokens = lambda m: set(m['name'].lower().split('_'))
        flag = lambda m, k: m['flags'].get(k, '')
        for default in (m for m in models if flag(m, 'cas.IsDefaultWearStyle') == 'yes'):
            designs = ids(default)
            members = [s for s in models if s['id'] != default['id'] and flag(s, 'cas.IsDefaultWearStyle') != 'yes'
                       and (flag(s, 'cas.WearStyle') or flag(s, 'cas.HatOrientation'))
                       and flag(s, 'cas.Gender') == flag(default, 'cas.Gender') and len(ids(s)) == len(designs)
                       and (any(i in designs for i in ids(s)) or tokens(default) <= tokens(s)
                            or default['name'].replace('closed', 'open') == s['name'])]
            if members:
                result[default['name']] = sorted(m['name'] for m in members)
    return result


def origin(name):
    """'s1' / 's2' for ported Skate 1 / Skate 2 models (every name token carries the prefix), '' for Skate 3."""
    return next((g for g in ('s1', 's2') if all(t.startswith(g) for t in name.split('_'))), '')


def skate2(name):
    return origin(name) == 's2'


def skate1(name):
    return origin(name) == 's1'


def crossings(catalog):
    return {d: ms for d, ms in families(catalog).items() if any(origin(m) != origin(d) for m in ms)}
