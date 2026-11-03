"""Fit map-picture pixels = (world x, world z) * k + offset by overlapping the picture's filled
city shape with the converted city's top-down footprint (best intersection-over-union)."""
import numpy as np
from PIL import Image, ImageFilter

CELL = 4.0  # world metres per footprint cell


def footprint(points):
    x, z = points[:, 0], points[:, 2]
    lo = np.array([x.min(), z.min()])
    size = np.ceil((np.array([x.max(), z.max()]) - lo) / CELL).astype(int) + 1
    grid = np.zeros(size[::-1], np.int32)
    np.add.at(grid, (((z - lo[1]) / CELL).astype(int), ((x - lo[0]) / CELL).astype(int)), 1)
    img = Image.fromarray(((grid > 2) * 255).astype(np.uint8))
    img = img.filter(ImageFilter.MaxFilter(5)).filter(ImageFilter.MinFilter(5))  # close small gaps
    return fill(np.asarray(img) > 0), lo


def fill(mask):
    """Mask plus its enclosed holes (flood the outside from the border)."""
    from collections import deque
    h, w = mask.shape
    outside = np.zeros_like(mask)
    q = deque([(y, x) for y in range(h) for x in (0, w - 1)] + [(y, x) for x in range(w) for y in (0, h - 1)])
    while q:
        y, x = q.popleft()
        if 0 <= y < h and 0 <= x < w and not outside[y, x] and not mask[y, x]:
            outside[y, x] = True
            q.extend(((y + 1, x), (y - 1, x), (y, x + 1), (y, x - 1)))
    return ~outside


def iou(map_mask, grid, lo, k, ox, oy):
    h, w = map_mask.shape
    ys, xs = np.mgrid[0:h, 0:w] + 0.5
    wx = (xs - ox) / k
    wz = (ys - oy) / k
    gx = ((wx - lo[0]) / CELL).astype(int)
    gz = ((wz - lo[1]) / CELL).astype(int)
    inside = (gx >= 0) & (gz >= 0) & (gx < grid.shape[1]) & (gz < grid.shape[0])
    g = np.zeros_like(map_mask)
    g[inside] = grid[gz[inside], gx[inside]]
    return (g & map_mask).sum() / max((g | map_mask).sum(), 1)


def fit(map_mask, points):
    grid, lo = footprint(points)
    ys, xs = np.nonzero(map_mask)
    wz, wx = np.nonzero(grid)
    world_w = (wx.max() - wx.min() + 1) * CELL
    map_w = xs.max() - xs.min() + 1
    k0 = map_w / world_w
    best = (-1, None)
    # coarse: scale around the bounding-box ratio, offsets aligning the shapes' centres
    for k in k0 * np.linspace(0.8, 1.25, 19):
        cx = xs.mean() - ((wx.mean() + 0.5) * CELL + lo[0]) * k
        cz = ys.mean() - ((wz.mean() + 0.5) * CELL + lo[1]) * k
        for dx in np.linspace(-40, 40, 17):
            for dz in np.linspace(-40, 40, 17):
                s = iou(map_mask, grid, lo, k, cx + dx, cz + dz)
                if s > best[0]:
                    best = (s, [k, cx + dx, cz + dz])
    # refine: coordinate descent with shrinking steps
    score, (k, ox, oy) = best
    steps = [k * 0.02, 4.0, 4.0]
    for _ in range(60):
        improved = False
        for i in range(3):
            for sign in (-1, 1):
                trial = [k, ox, oy]
                trial[i] += sign * steps[i]
                s = iou(map_mask, grid, lo, *trial)
                if s > score:
                    score, (k, ox, oy), improved = s, trial, True
        if not improved:
            steps = [s / 2 for s in steps]
            if steps[1] < 0.1:
                break
    return score, (k, ox, oy)
