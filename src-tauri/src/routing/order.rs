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

#[cfg(test)]
mod tests {
    use super::*;

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
