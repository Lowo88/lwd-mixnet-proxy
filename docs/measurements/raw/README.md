# Raw benchmark output

Unedited stdout from `lwd-mixnet-bench`, one file per run, kept so the conclusions in
[`../2026-08-06-probe-and-retry.md`](../2026-08-06-probe-and-retry.md) can be checked against the
trials they came from rather than taken on trust. Numbered in the order the runs happened.

| file | what it was | verdict |
|---|---|---|
| `2026-08-06-run1-sequential.txt` | 300 trials, rounds of 1 | **invalid past trial ~200**: the host began suspending and the client started refusing opens |
| `2026-08-06-run2-hedged-only.txt` | 200 trials, rounds of 3 alone | **unattributable**: no arm to compare against in the same window, and a per-stream rate is not measurable at this round size |
| `2026-08-06-run3-interleaved.txt` | 300 trials, rounds of 1 and 3 interleaved | **the result**: no refused opens, no host suspension |
| `2026-08-06-run4-warm-traffic.txt` | 200 trials, gaps of 0/2/8/20 s, stopped at 148 | **valid to trial ~137**, where the host began suspending |

Each line is one trial: its number, the arm, whether any stream answered, how many rounds it took,
how many opened streams went unanswered, how many opens the local client refused, and how long the
whole trial took. The summary at the foot of each file is what the tool printed at the time, so runs
1 and 2 still show the headline numbers that turned out to be misleading; what was wrong with them,
and how the tool was changed so it says so itself, is in the report.

**One edit was made to these files**: the Nym address each run dialled has been replaced with a
stable label, so `server-A`, `server-B` and `server-C` stand for three different serving halves and
runs 2 and 3 can be seen to have shared one. The addresses themselves were of no use here. They
appeared once per file, in the header, and no number depends on them; the identities were ephemeral
and died with their processes; and the last component of such an address names a **gateway**, a
public node run by someone else. Publishing a failure rate next to a stranger's node would imply
something about it that these runs never measured.

Not kept: the SDK's own stderr. It is large, ANSI-coloured and mostly routine, and the two findings
drawn from it are quoted in the report (`Not enough bandwidth` six minutes into run 1, and 754
`sending_delay_controller` warnings during it).

Reproducing any of these needs the same release binaries, a running `lwd-mixnet-server`, and a host
kept awake. The absolute rates will not reproduce: they are one sample of a distribution that has
been seen anywhere between 2% and 51%, which is the reason every comparison here is interleaved
rather than run as separate sessions.
