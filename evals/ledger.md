# Eval Ledger

Durable record of eval suite runs — one row per completed
`syscity eval run` (appended automatically, best-effort).

Why a file: files and git history survive context compaction and CI log
expiry; conversation memory does not. When judging "which version is best",
compare against this ledger, not memory. See `docs/harness.md` §8.

| Date | Commit | Suite | Executor | Judge | Mode | Passed | Rate | Gate | Failed tasks |
|------|--------|-------|----------|-------|------|--------|------|------|--------------|
