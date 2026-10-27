const EDIT_STEPS_COUNT_MAX: isize = 1000;

#[derive(Clone, Copy)]
enum Step { Keep, Delete(usize), Insert(usize) }

pub fn changed_lines(a: &[&str], b: &[&str]) -> (Vec<usize>, Vec<usize>) {
    let (mut old, mut new) = (Vec::new(), Vec::new());
    for s in edits(a, b) {
        match s { Step::Delete(i) => old.push(i), Step::Insert(j) => new.push(j), Step::Keep => {} }
    }
    (old, new)
}
fn replaced(pre: usize, old: usize, new: usize, suf: usize) -> Vec<Step> {
    std::iter::repeat_n(Step::Keep, pre).chain((pre..pre + old).map(Step::Delete)).chain((pre..pre + new).map(Step::Insert)).chain(std::iter::repeat_n(Step::Keep, suf)).collect()
}
fn edits(a: &[&str], b: &[&str]) -> Vec<Step> {
    let mut pre = 0;
    while pre < a.len() && pre < b.len() && a[pre] == b[pre] { pre += 1; }
    let mut suf = 0;
    while suf < a.len() - pre && suf < b.len() - pre && a[a.len() - 1 - suf] == b[b.len() - 1 - suf] { suf += 1; }
    let (x, y) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    let (n, m) = (x.len() as isize, y.len() as isize);
    let max = (n + m) as usize;
    let off = max as isize + 1;
    let mut v = vec![0isize; 2 * max + 3];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut end = None;
    'search: for d in 0..=(max as isize).min(EDIT_STEPS_COUNT_MAX) {
        trace.push(v[(off - d - 1) as usize..=(off + d + 1) as usize].to_vec());
        for k in (-d..=d).step_by(2) {
            let i = (k + off) as usize;
            let mut px = if k == -d || (k != d && v[i - 1] < v[i + 1]) { v[i + 1] } else { v[i - 1] + 1 };
            let mut py = px - k;
            while px < n && py < m && x[px as usize] == y[py as usize] { px += 1; py += 1; }
            v[i] = px;
            if px >= n && py >= m { end = Some(d); break 'search; }
        }
    }
    let Some(end) = end else { return replaced(pre, x.len(), y.len(), suf) };
    let (mut px, mut py, mut steps) = (n, m, vec![Step::Keep; suf]);
    for d in (0..=end).rev() {
        let w = &trace[d as usize];
        let at = |k: isize| w[(k + d + 1) as usize];
        let k = px - py;
        let prev = if k == -d || (k != d && at(k - 1) < at(k + 1)) { k + 1 } else { k - 1 };
        let (qx, qy) = (at(prev), at(prev) - prev);
        while px > qx && py > qy { steps.push(Step::Keep); px -= 1; py -= 1; }
        if d > 0 { steps.push(if px == qx { Step::Insert(pre + (py - 1) as usize) } else { Step::Delete(pre + (px - 1) as usize) }); }
        px = qx;
        py = qy;
    }
    steps.extend(std::iter::repeat_n(Step::Keep, pre));
    steps.reverse();
    steps
}
