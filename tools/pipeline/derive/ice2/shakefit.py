exec(open('shakecheck.py').read().split('worst = 0')[0])
import itertools
random.seed(1)
cases = []
for _ in range(30):
    V = {p: [random.uniform(-0.4, 0.4), random.uniform(-0.4, 0.4)] for p in range(0x51, 0x57)}
    S, u = random.uniform(0, 1), random.random()
    mem = run(0x82842308, 0x82842580, {28: 0, 29: 1, 1: 0}, {1: 0.5, 29: S * 0.5, 30: u, 31: 0.}, lambda p, i: V[p][i])
    cases.append((V, S, u, [[mem[0x70 + 16 * i + 4 * j] for j in range(3)] for i in range(3)]))
best = None
for perm in itertools.permutations(range(3)):
    for signs in itertools.product((1, -1), repeat=3):
        for transpose in (0, 1):
            err = 0
            for V, S, u, M in cases:
                def q(i):
                    v = [V[p][i] * S for p in (0x51, 0x52, 0x53)]
                    return v + [math.sqrt(1 - math.sqrt(sum(c * c for c in v)))]
                a, b = q(0), q(1)
                l = [a[k] + (b[k] - a[k]) * u for k in range(4)]
                comp = [signs[k] * l[perm[k]] for k in range(3)]
                R = rstd(*comp, l[3])
                if transpose: R = [list(r) for r in zip(*R)]
                err = max(err, max(abs(M[i][j] - R[i][j]) for i in range(3) for j in range(3)))
            if best is None or err < best[0]: best = (err, perm, signs, transpose)
print(best)
