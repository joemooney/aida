# AIDA agent-surface benchmark results

Generated: 2026-06-29T04:01:31.090642+00:00

## Per-condition summary

| Condition | Runs | Success% | Avg in-tok | Avg out-tok | Avg cost | Total cost | Avg turns | Avg tools | Avg s |
|---|---|---|---|---|---|---|---|---|---|
| cli | 18 | 100% | 55853 | 354 | $0.0358 | $0.6443 | 3.1 | 2.1 | 11.9 |
| mcp | 18 | 89% | 129015 | 350 | $0.0709 | $1.2766 | 3.1 | 2.1 | 11.0 |
| mcp-toolsearch | 18 | 100% | 86938 | 674 | $0.0636 | $1.1455 | 4.8 | 3.8 | 17.0 |
| toon | 18 | 100% | 55988 | 347 | $0.0360 | $0.6472 | 2.8 | 1.8 | 11.7 |

## Single-call vs chained (multi-round) split

| Condition | Bucket | Runs | Success% | Avg in-tok | Avg cost | Avg turns | Avg tools |
|---|---|---|---|---|---|---|---|
| cli | single-call | 15 | 100% | 44122 | $0.0288 | 2.2 | 1.2 |
| cli | chained | 3 | 100% | 114510 | $0.0705 | 7.3 | 6.3 |
| mcp | single-call | 15 | 87% | 102347 | $0.0626 | 2.3 | 1.4 |
| mcp | chained | 3 | 100% | 262357 | $0.1123 | 6.7 | 5.7 |
| mcp-toolsearch | single-call | 15 | 100% | 70461 | $0.0489 | 4.0 | 3.0 |
| mcp-toolsearch | chained | 3 | 100% | 169320 | $0.1372 | 8.7 | 7.7 |
| toon | single-call | 15 | 100% | 44324 | $0.0292 | 2.2 | 1.2 |
| toon | chained | 3 | 100% | 114308 | $0.0699 | 6.0 | 5.0 |

## Per-task breakdown

### next_queue_item

| Condition | Avg in-tok | Avg out-tok | Avg cost | Avg turns | Success |
|---|---|---|---|---|---|
| cli | 44020 | 132 | $0.0272 | 2.0 | 3/3 |
| mcp | 96745 | 110 | $0.0415 | 2.0 | 3/3 |
| mcp-toolsearch | 60061 | 241 | $0.0385 | 3.0 | 3/3 |
| toon | 44248 | 138 | $0.0273 | 2.0 | 3/3 |

### show_spec_blocked_by

| Condition | Avg in-tok | Avg out-tok | Avg cost | Avg turns | Success |
|---|---|---|---|---|---|
| cli | 44378 | 295 | $0.0317 | 3.0 | 3/3 |
| mcp | 97200 | 325 | $0.0477 | 3.0 | 3/3 |
| mcp-toolsearch | 60813 | 486 | $0.0457 | 4.0 | 3/3 |
| toon | 44461 | 311 | $0.0315 | 3.0 | 3/3 |

### status_snapshot

| Condition | Avg in-tok | Avg out-tok | Avg cost | Avg turns | Success |
|---|---|---|---|---|---|
| cli | 44191 | 294 | $0.0306 | 2.0 | 3/3 |
| mcp | 96826 | 196 | $0.0436 | 2.0 | 3/3 |
| mcp-toolsearch | 62112 | 523 | $0.0505 | 4.0 | 3/3 |
| toon | 44398 | 272 | $0.0306 | 2.0 | 3/3 |

### find_finding

| Condition | Avg in-tok | Avg out-tok | Avg cost | Avg turns | Success |
|---|---|---|---|---|---|
| cli | 44012 | 173 | $0.0273 | 2.0 | 3/3 |
| mcp | 124092 | 304 | $0.1363 | 2.7 | 1/3 |
| mcp-toolsearch | 108259 | 788 | $0.0659 | 6.0 | 3/3 |
| toon | 44237 | 172 | $0.0281 | 2.0 | 3/3 |

### file_spec

| Condition | Avg in-tok | Avg out-tok | Avg cost | Avg turns | Success |
|---|---|---|---|---|---|
| cli | 44010 | 172 | $0.0274 | 2.0 | 3/3 |
| mcp | 96870 | 212 | $0.0440 | 2.0 | 3/3 |
| mcp-toolsearch | 61061 | 386 | $0.0441 | 3.0 | 3/3 |
| toon | 44276 | 175 | $0.0283 | 2.0 | 3/3 |

### chained_followup

| Condition | Avg in-tok | Avg out-tok | Avg cost | Avg turns | Success |
|---|---|---|---|---|---|
| cli | 114510 | 1061 | $0.0705 | 7.3 | 3/3 |
| mcp | 262357 | 953 | $0.1123 | 6.7 | 3/3 |
| mcp-toolsearch | 169320 | 1622 | $0.1372 | 8.7 | 3/3 |
| toon | 114308 | 1011 | $0.0699 | 6.0 | 3/3 |

