from shakeemu import *
import random
def rstd(qa, qb, qc, w):
    return [[1 - 2 * (qb * qb + qc * qc), 2 * (qa * qb - qc * w), 2 * (qa * qc + qb * w)],
            [2 * (qa * qb + qc * w), 1 - 2 * (qa * qa + qc * qc), 2 * (qb * qc - qa * w)],
            [2 * (qa * qc - qb * w), 2 * (qb * qc + qa * w), 1 - 2 * (qa * qa + qb * qb)]]
def model(V, S, u):
    def q(i):
        x, y, z = (V[p][i] * S for p in (0x51, 0x52, 0x53))
        return x, y, z, math.sqrt(1 - math.sqrt(x * x + y * y + z * z))
    a, b = q(0), q(1)
    x, y, z, w = (a[k] + (b[k] - a[k]) * u for k in range(4))
    return rstd(z, -x, y, w), [-V[0x56][0] * S, -V[0x54][0] * S, V[0x55][0] * S]
worst = 0
for _ in range(200):
    V = {p: [random.uniform(-0.4, 0.4), random.uniform(-0.4, 0.4)] for p in range(0x51, 0x57)}
    S, u = random.uniform(0, 1), random.random()
    mem = run(0x82842308, 0x82842580, {28: 0, 29: 1, 1: 0}, {1: 0.5, 29: S * 0.5, 30: u, 31: 0.}, lambda p, i: V[p][i])
    M = [[mem[0x70 + 16 * i + 4 * j] for j in range(3)] for i in range(3)]
    R, t = model(V, S, u)
    worst = max(worst, max(abs(M[i][j] - R[i][j]) for i in range(3) for j in range(3)), max(abs(mem[0x50 + 4 * k] - t[k]) for k in range(3)))
print('worst', worst)
