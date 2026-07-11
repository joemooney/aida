# AIDA agent-surface benchmark results

Generated: 2026-07-11T16:26:04.040532+00:00

## Per-condition summary

| Condition | Runs | Success% | Avg in-tok | Avg out-tok | Avg cost | Total cost | Avg turns | Avg tools | Avg s |
|---|---|---|---|---|---|---|---|---|---|
| cli | 18 | 89% | 95220 | 547 | $0.0504 | $0.9072 | 3.1 | 2.1 | 11.9 |
| mcp | 18 | 100% | 204505 | 385 | $0.1165 | $2.0971 | 3.4 | 2.5 | 10.0 |
| mcp-toolsearch | 18 | 100% | 118466 | 524 | $0.0693 | $1.2482 | 4.4 | 3.4 | 12.0 |
| toon | 18 | 100% | 89783 | 376 | $0.0416 | $0.7488 | 2.9 | 1.9 | 9.5 |

## Single-call vs chained (multi-round) split

| Condition | Bucket | Runs | Success% | Avg in-tok | Avg cost | Avg turns | Avg tools |
|---|---|---|---|---|---|---|---|
| cli | single-call | 15 | 100% | 68750 | $0.0318 | 2.4 | 1.4 |
| cli | chained | 3 | 33% | 227568 | $0.1433 | 6.3 | 5.3 |
| mcp | single-call | 15 | 100% | 168526 | $0.1111 | 2.7 | 1.9 |
| mcp | chained | 3 | 100% | 384398 | $0.1437 | 6.7 | 5.7 |
| mcp-toolsearch | single-call | 15 | 100% | 97493 | $0.0580 | 3.7 | 2.7 |
| mcp-toolsearch | chained | 3 | 100% | 223332 | $0.1261 | 8.0 | 7.0 |
| toon | single-call | 15 | 100% | 71075 | $0.0322 | 2.3 | 1.3 |
| toon | chained | 3 | 100% | 183323 | $0.0884 | 5.7 | 4.7 |

## Per-task breakdown

### next_queue_item

| Condition | Avg in-tok | Avg out-tok | Avg cost | Avg turns | Success |
|---|---|---|---|---|---|
| cli | 62210 | 96 | $0.0341 | 2.0 | 3/3 |
| mcp | 133712 | 72 | $0.1691 | 2.0 | 3/3 |
| mcp-toolsearch | 85414 | 227 | $0.0880 | 3.0 | 3/3 |
| toon | 62496 | 92 | $0.0345 | 2.0 | 3/3 |

### show_spec_blocked_by

| Condition | Avg in-tok | Avg out-tok | Avg cost | Avg turns | Success |
|---|---|---|---|---|---|
| cli | 62699 | 241 | $0.0293 | 2.0 | 3/3 |
| mcp | 172446 | 435 | $0.1995 | 3.7 | 3/3 |
| mcp-toolsearch | 86175 | 391 | $0.0462 | 4.0 | 3/3 |
| toon | 62739 | 176 | $0.0266 | 2.0 | 3/3 |

### status_snapshot

| Condition | Avg in-tok | Avg out-tok | Avg cost | Avg turns | Success |
|---|---|---|---|---|---|
| cli | 62632 | 376 | $0.0299 | 3.0 | 3/3 |
| mcp | 133838 | 135 | $0.0461 | 2.0 | 3/3 |
| mcp-toolsearch | 87590 | 450 | $0.0509 | 3.3 | 3/3 |
| toon | 73330 | 384 | $0.0344 | 2.7 | 3/3 |

### find_finding

| Condition | Avg in-tok | Avg out-tok | Avg cost | Avg turns | Success |
|---|---|---|---|---|---|
| cli | 93867 | 293 | $0.0394 | 3.0 | 3/3 |
| mcp | 268742 | 403 | $0.0931 | 4.0 | 3/3 |
| mcp-toolsearch | 142277 | 470 | $0.0620 | 5.0 | 3/3 |
| toon | 94260 | 337 | $0.0402 | 3.0 | 3/3 |

### file_spec

| Condition | Avg in-tok | Avg out-tok | Avg cost | Avg turns | Success |
|---|---|---|---|---|---|
| cli | 62344 | 184 | $0.0263 | 2.0 | 3/3 |
| mcp | 133894 | 216 | $0.0475 | 2.0 | 3/3 |
| mcp-toolsearch | 86011 | 288 | $0.0429 | 3.0 | 3/3 |
| toon | 62550 | 132 | $0.0255 | 2.0 | 3/3 |

### chained_followup

| Condition | Avg in-tok | Avg out-tok | Avg cost | Avg turns | Success |
|---|---|---|---|---|---|
| cli | 227568 | 2093 | $0.1433 | 6.3 | 1/3 |
| mcp | 384398 | 1046 | $0.1437 | 6.7 | 3/3 |
| mcp-toolsearch | 223332 | 1317 | $0.1261 | 8.0 | 3/3 |
| toon | 183323 | 1131 | $0.0884 | 5.7 | 3/3 |

