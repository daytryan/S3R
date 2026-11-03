from shakeemu import *
import random
# values: param -> [frame a, frame b]
V = {0x51: [0.1, 0.3], 0x52: [0.2, -0.1], 0x53: [-0.05, 0.15], 0x54: [0.4, 0.2], 0x55: [-0.3, 0.1], 0x56: [0.25, -0.2]}
S, u = 0.7, 0.25
r = {28: 0, 29: 1, 1: 0}
f = {1: 0.5, 29: S * 0.5, 30: u, 31: 0.}
mem = run(0x82842308, 0x82842580, r, f, lambda p, i: V[p][i])
M = [[mem[0x70 + 16 * i + 4 * j] for j in range(4)] for i in range(4)]
for row in M: print([round(x, 5) for x in row])
print('vec', [round(mem[0x50 + 4 * k], 5) for k in range(4)])
lerp = lambda p: V[p][0] + (V[p][1] - V[p][0]) * u
print('lerp QX QY QZ', [round(lerp(p) * S, 5) for p in (0x51, 0x52, 0x53)], 'POS XYZ', [round(lerp(p) * S, 5) for p in (0x54, 0x55, 0x56)])
