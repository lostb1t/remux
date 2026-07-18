#!/usr/bin/env python3
"""Compare two latency samples robustly: quantiles + Mann-Whitney U (normal
approximation, tie-corrected) + bootstrap CI on the median ratio. No scipy."""
import sys, math, random, statistics

def q(v, p):
    if not v: return float('nan')
    k = (len(v)-1)*p; f = math.floor(k); c = math.ceil(k)
    return v[f] if f == c else v[f]*(c-k) + v[c]*(k-f)

def mannwhitney(a, b):
    n1, n2 = len(a), len(b)
    comb = sorted([(x,0) for x in a] + [(x,1) for x in b])
    ranks = [0.0]*len(comb); i = 0
    while i < len(comb):
        j = i
        while j+1 < len(comb) and comb[j+1][0] == comb[i][0]: j += 1
        r = (i+j)/2 + 1
        for k in range(i, j+1): ranks[k] = r
        i = j+1
    r1 = sum(ranks[i] for i,(_,g) in enumerate(comb) if g == 0)
    u1 = r1 - n1*(n1+1)/2
    mu = n1*n2/2
    # tie correction
    tie = 0; i = 0
    while i < len(comb):
        j = i
        while j+1 < len(comb) and comb[j+1][0] == comb[i][0]: j += 1
        t = j-i+1; tie += t**3 - t; i = j+1
    N = n1+n2
    var = n1*n2/12 * ((N+1) - tie/(N*(N-1))) if N > 1 else 0
    if var <= 0: return 1.0
    z = (u1-mu)/math.sqrt(var)
    return math.erfc(abs(z)/math.sqrt(2))  # two-sided p

def boot_ratio(a, b, iters=2000):
    random.seed(12345); out = []
    for _ in range(iters):
        ma = statistics.median(random.choices(a, k=len(a)))
        mb = statistics.median(random.choices(b, k=len(b)))
        if mb: out.append(ma/mb)
    out.sort()
    return out[int(.025*len(out))], out[int(.975*len(out))]

def load(f):
    return sorted(float(x.split()[-1]) for x in open(f) if x.strip())

if __name__ == "__main__":
    label, fa, fb = sys.argv[1], sys.argv[2], sys.argv[3]
    a, b = load(fa), load(fb)
    ms = lambda x: x*1000
    print(f"\n=== {label} ===")
    for nm, v in (("baseline", a), ("treatment", b)):
        print(f"  {nm:<10} n={len(v):<4} min={ms(v[0]):8.1f}  p10={ms(q(v,.10)):8.1f}  "
              f"median={ms(statistics.median(v)):8.1f}  p90={ms(q(v,.90)):8.1f}  max={ms(v[-1]):8.1f} ms")
    ratio = statistics.median(a)/statistics.median(b)
    lo, hi = boot_ratio(a, b)
    p = mannwhitney(a, b)
    verdict = ("TREATMENT FASTER" if lo > 1.05 else
               "TREATMENT SLOWER" if hi < 0.95 else "NO DIFFERENCE RESOLVED")
    print(f"  median ratio base/treat = {ratio:.2f}x  (95% CI {lo:.2f}-{hi:.2f})  p={p:.4f}")
    print(f"  VERDICT: {verdict}")
