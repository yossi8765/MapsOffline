//! Visiting order of a handful of stops by a travel-time matrix (Held–Karp dynamic programme;
//! n <= 10 so 2^n * n^2 is tiny). Index 0 is the fixed origin.
use std::collections::HashMap;

/// Returns a permutation of 0..n starting with 0. With `fixed_end` the last index stays last.
pub fn best_order(cost: &[Vec<f64>], fixed_end: bool) -> Vec<usize> {
    let n = cost.len();
    if n <= 2 {
        return (0..n).collect();
    }
    let middle: Vec<usize> = if fixed_end { (1..n - 1).collect() } else { (1..n).collect() };
    let m = middle.len();
    if m == 0 {
        return (0..n).collect();
    }
    // dp[(mask, last)] = (cost, prev_last)
    let mut dp: HashMap<(u32, usize), (f64, usize)> = HashMap::new();
    for (i, &p) in middle.iter().enumerate() {
        dp.insert((1 << i, i), (cost[0][p], usize::MAX));
    }
    for mask in 1u32..(1 << m) {
        for last in 0..m {
            if mask & (1 << last) == 0 {
                continue;
            }
            let Some(&(c, _)) = dp.get(&(mask, last)) else { continue };
            for next in 0..m {
                if mask & (1 << next) != 0 {
                    continue;
                }
                let nc = c + cost[middle[last]][middle[next]];
                let key = (mask | (1 << next), next);
                if dp.get(&key).is_none_or(|&(old, _)| nc < old) {
                    dp.insert(key, (nc, last));
                }
            }
        }
    }
    let full = (1u32 << m) - 1;
    let mut best: Option<(f64, usize)> = None;
    for last in 0..m {
        if let Some(&(c, _)) = dp.get(&(full, last)) {
            let total = if fixed_end { c + cost[middle[last]][n - 1] } else { c };
            if best.is_none_or(|(b, _)| total < b) {
                best = Some((total, last));
            }
        }
    }
    let Some((_, mut last)) = best else { return (0..n).collect() };
    let mut mask = full;
    let mut seq = Vec::with_capacity(m);
    while last != usize::MAX {
        seq.push(middle[last]);
        let (_, prev) = dp[&(mask, last)];
        mask &= !(1 << last);
        last = prev;
    }
    seq.reverse();
    let mut out = vec![0];
    out.extend(seq);
    if fixed_end {
        out.push(n - 1);
    }
    out
}

/// One stop's constraints for [`plan_trip`]: minutes spent there and an arrival window
/// (seconds on the same clock as `start`).
#[derive(Clone, Copy, Debug, Default)]
pub struct StopRule {
    pub stay_sec: f64,
    pub window: (Option<f64>, Option<f64>),
}

/// Lateness weight: a second late at a stop costs as much as this many seconds of driving.
const LATE_WEIGHT: f64 = 10.0;

/// (objective, arrival time at each position) of visiting `order` (starting with 0).
pub fn schedule(cost: &[Vec<f64>], order: &[usize], start: f64, rules: &[StopRule]) -> (f64, Vec<f64>) {
    let mut t = start;
    let mut late = 0.0;
    let mut arrivals = Vec::with_capacity(order.len());
    for (k, &i) in order.iter().enumerate() {
        if k > 0 {
            t += cost[order[k - 1]][i];
        }
        let r = rules.get(i).copied().unwrap_or_default();
        if let Some(open) = r.window.0 {
            if t < open {
                t = open; // wait for the window to open
            }
        }
        if let Some(close) = r.window.1 {
            late += (t - close).max(0.0);
        }
        arrivals.push(t);
        if k > 0 {
            t += r.stay_sec;
        }
    }
    (t - start + late * LATE_WEIGHT, arrivals)
}

/// Visiting order with stay times and arrival windows: exact (Held–Karp on the finishing time)
/// up to 12 movable stops, nearest neighbour + 2-opt / or-opt above. Returns the order and the
/// arrival time at each position.
pub fn plan_trip(cost: &[Vec<f64>], fixed_end: bool, start: f64, rules: &[StopRule]) -> (Vec<usize>, Vec<f64>) {
    let n = cost.len();
    if n <= 2 {
        let order: Vec<usize> = (0..n).collect();
        let (_, arr) = schedule(cost, &order, start, rules);
        return (order, arr);
    }
    let middle: Vec<usize> = if fixed_end { (1..n - 1).collect() } else { (1..n).collect() };
    let m = middle.len();
    let finish = |seq: &[usize]| -> Vec<usize> {
        let mut o = vec![0];
        o.extend_from_slice(seq);
        if fixed_end {
            o.push(n - 1);
        }
        o
    };
    let eval = |seq: &[usize]| schedule(cost, &finish(seq), start, rules).0;
    let best_seq: Vec<usize> = if m <= 12 {
        // dp[mask][last] = (objective so far, time, previous last)
        let size = 1usize << m;
        let mut dp = vec![(f64::INFINITY, 0.0f64, usize::MAX); size * m];
        let step = |t: f64, from: usize, to: usize, late: &mut f64| -> f64 {
            let mut t = t + cost[from][to];
            let r = rules.get(to).copied().unwrap_or_default();
            if let Some(open) = r.window.0 {
                t = t.max(open);
            }
            if let Some(close) = r.window.1 {
                *late += (t - close).max(0.0);
            }
            t + r.stay_sec
        };
        for i in 0..m {
            let mut late = 0.0;
            let t = step(start, 0, middle[i], &mut late);
            dp[(1 << i) * m + i] = (t - start + late * LATE_WEIGHT, t, usize::MAX);
        }
        for mask in 1..size {
            for last in 0..m {
                let (obj, t, _) = dp[mask * m + last];
                if mask & (1 << last) == 0 || !obj.is_finite() {
                    continue;
                }
                for next in 0..m {
                    if mask & (1 << next) != 0 {
                        continue;
                    }
                    let mut late = 0.0;
                    let t2 = step(t, middle[last], middle[next], &mut late);
                    let o2 = obj + (t2 - t) + late * LATE_WEIGHT;
                    let k = (mask | (1 << next)) * m + next;
                    if o2 < dp[k].0 {
                        dp[k] = (o2, t2, last);
                    }
                }
            }
        }
        let full = size - 1;
        let mut best: Option<(f64, usize)> = None;
        for last in 0..m {
            let (obj, t, _) = dp[full * m + last];
            if !obj.is_finite() {
                continue;
            }
            let total = if fixed_end {
                let mut late = 0.0;
                let t2 = step(t, middle[last], n - 1, &mut late);
                obj + (t2 - t) + late * LATE_WEIGHT
            } else {
                obj
            };
            if best.is_none_or(|(b, _)| total < b) {
                best = Some((total, last));
            }
        }
        match best {
            None => middle.clone(),
            Some((_, mut last)) => {
                let mut mask = full;
                let mut seq = Vec::with_capacity(m);
                while last != usize::MAX {
                    seq.push(middle[last]);
                    let prev = dp[mask * m + last].2;
                    mask &= !(1 << last);
                    last = prev;
                }
                seq.reverse();
                seq
            }
        }
    } else {
        // nearest neighbour, then local search
        let mut seq = Vec::with_capacity(m);
        let mut left: Vec<usize> = middle.clone();
        let mut cur = 0usize;
        while !left.is_empty() {
            let (k, _) = left.iter().enumerate().min_by(|a, b| cost[cur][*a.1].total_cmp(&cost[cur][*b.1])).unwrap();
            cur = left.swap_remove(k);
            seq.push(cur);
        }
        let mut best = eval(&seq);
        let mut improved = true;
        let mut rounds = 0;
        while improved && rounds < 50 {
            improved = false;
            rounds += 1;
            // 2-opt: reverse a stretch
            for i in 0..m {
                for j in i + 1..m {
                    let mut c = seq.clone();
                    c[i..=j].reverse();
                    let v = eval(&c);
                    if v + 1e-6 < best {
                        best = v;
                        seq = c;
                        improved = true;
                    }
                }
            }
            // or-opt: move one stop elsewhere
            for i in 0..m {
                for j in 0..m {
                    if i == j {
                        continue;
                    }
                    let mut c = seq.clone();
                    let x = c.remove(i);
                    c.insert(j, x);
                    let v = eval(&c);
                    if v + 1e-6 < best {
                        best = v;
                        seq = c;
                        improved = true;
                    }
                }
            }
        }
        seq
    };
    let order = finish(&best_seq);
    let (_, arr) = schedule(cost, &order, start, rules);
    (order, arr)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Brute force over all permutations of the movable stops.
    fn brute(cost: &[Vec<f64>], fixed_end: bool, rules: &[StopRule]) -> f64 {
        let n = cost.len();
        let middle: Vec<usize> = if fixed_end { (1..n - 1).collect() } else { (1..n).collect() };
        fn perms(v: &mut Vec<usize>, k: usize, out: &mut Vec<Vec<usize>>) {
            if k == v.len() {
                out.push(v.clone());
                return;
            }
            for i in k..v.len() {
                v.swap(k, i);
                perms(v, k + 1, out);
                v.swap(k, i);
            }
        }
        let mut all = Vec::new();
        perms(&mut middle.clone(), 0, &mut all);
        all.into_iter()
            .map(|seq| {
                let mut o = vec![0];
                o.extend(seq);
                if fixed_end {
                    o.push(n - 1);
                }
                schedule(cost, &o, 0.0, rules).0
            })
            .fold(f64::INFINITY, f64::min)
    }

    #[test]
    fn trip_matches_brute_force() {
        let mut seed = 0x1234_5678_9abc_def0u64;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 1000) as f64
        };
        for n in 3..=7 {
            for round in 0..6 {
                let pts: Vec<(f64, f64)> = (0..n).map(|_| (rnd(), rnd())).collect();
                let cost: Vec<Vec<f64>> = (0..n)
                    .map(|a| (0..n).map(|b| if a == b { 0.0 } else { ((pts[a].0 - pts[b].0).hypot(pts[a].1 - pts[b].1)) * (1.0 + (a * 7 + b) as f64 % 3.0 * 0.1) }).collect())
                    .collect();
                let rules: Vec<StopRule> = (0..n)
                    .map(|i| StopRule {
                        stay_sec: if round % 2 == 0 { rnd() / 10.0 } else { 0.0 },
                        window: if round % 3 == 0 && i > 0 { (Some(rnd()), Some(rnd() + 1500.0)) } else { (None, None) },
                    })
                    .collect();
                for fixed_end in [false, true] {
                    let (order, arr) = plan_trip(&cost, fixed_end, 0.0, &rules);
                    assert_eq!(order.len(), n);
                    assert_eq!(arr.len(), n);
                    assert_eq!(order[0], 0);
                    if fixed_end {
                        assert_eq!(*order.last().unwrap(), n - 1);
                    }
                    let got = schedule(&cost, &order, 0.0, &rules).0;
                    let want = brute(&cost, fixed_end, &rules);
                    assert!((got - want).abs() < 1e-6, "n={n} round={round} fixed_end={fixed_end}: {got} vs {want}");
                }
            }
        }
    }

    #[test]
    fn heuristic_for_many_stops() {
        // 20 stops on a line visited out of order: the plan must sort them
        let n = 20;
        let xs: Vec<f64> = (0..n).map(|i| if i == 0 { 0.0 } else { ((i * 7) % 19 + 1) as f64 }).collect();
        let cost: Vec<Vec<f64>> = (0..n).map(|a| (0..n).map(|b| (xs[a] - xs[b]).abs() * 60.0).collect()).collect();
        let (order, _) = plan_trip(&cost, false, 0.0, &[]);
        let total: f64 = order.windows(2).map(|w| cost[w[0]][w[1]]).sum();
        assert!((total - 19.0 * 60.0).abs() < 1e-6, "{order:?} {total}");
    }

    fn m(rows: &[&[f64]]) -> Vec<Vec<f64>> {
        rows.iter().map(|r| r.to_vec()).collect()
    }

    #[test]
    fn reorders_middle_stops() {
        // 0 -> far 1 -> near 2: better 0,2,1 when the end is free; 0,2,1,3 with a fixed end 3
        let c = m(&[&[0.0, 10.0, 1.0, 5.0], &[10.0, 0.0, 1.0, 1.0], &[1.0, 1.0, 0.0, 9.0], &[5.0, 1.0, 9.0, 0.0]]);
        assert_eq!(best_order(&c, true), vec![0, 2, 1, 3]);
        assert_eq!(best_order(&c, false), vec![0, 2, 1, 3]);
        let c2 = m(&[&[0.0, 1.0, 10.0], &[1.0, 0.0, 10.0], &[10.0, 10.0, 0.0]]);
        assert_eq!(best_order(&c2, false), vec![0, 1, 2]);
        assert_eq!(best_order(&c2, true), vec![0, 1, 2]);
        assert_eq!(best_order(&m(&[&[0.0, 1.0], &[1.0, 0.0]]), true), vec![0, 1]);
    }
}
