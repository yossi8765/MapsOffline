/** [west, south, east, north] in degrees. */
export type Bbox = [number, number, number, number];

/** `b` grown by `factor` of its size on every side (0.25 = a box 1.5× as wide and high). */
export function padBbox(b: Bbox, factor: number): Bbox {
  const dx = (b[2] - b[0]) * factor;
  const dy = (b[3] - b[1]) * factor;
  return [b[0] - dx, b[1] - dy, b[2] + dx, b[3] + dy];
}

/** `inner` lies completely inside `outer`. */
export function containsBbox(outer: Bbox, inner: Bbox): boolean {
  return inner[0] >= outer[0] && inner[1] >= outer[1] && inner[2] <= outer[2] && inner[3] <= outer[3];
}

/** Bounding box of points (null for none). */
export function bboxOf(points: [number, number][]): Bbox | null {
  if (!points.length) return null;
  let w = Infinity,
    s = Infinity,
    e = -Infinity,
    n = -Infinity;
  for (const [x, y] of points) {
    if (x < w) w = x;
    if (x > e) e = x;
    if (y < s) s = y;
    if (y > n) n = y;
  }
  return [w, s, e, n];
}
