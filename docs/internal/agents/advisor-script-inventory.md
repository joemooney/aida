# Advisor script inventory

<!-- trace:STORY-2571 | ai:codex -->

Snapshot: 2026-11-06, host directory
`~/.local/share/aida/ops/advisor-scripts/*.sh` (direct children).
This records all shell scripts present, including historical/generated jobs,
rather than guessing which files are unused from sandbox process visibility.
A file's presence, timestamp or incoming reference does not prove it is running.
No live service was stopped, restarted or modified to prepare this inventory.
Dispositions follow the
[architecture sketch's v2 signoff](../../plans/2026-11-06-story-2571-advisor-operations-sketch.md)
(2026-11-06 19:56 UTC). Migration and runtime evidence remain separate work;
signoff does not mean these scripts have been replaced or retired.

The inventory reads each script and records its full SHA-256. It deliberately
does not copy scripts, environment contents, credentials or transcripts into the
repository. Recheck hashes and enumerate added files before migration; the host
directory can change while lanes run. No automatic deletion is authorized.

Round-2 addendum (2026-11-06): the original 11:16 snapshot below is retained.
Two later arrivals inspected for this revision are listed separately with their
hashes. `host-merge.sh` also changed: its inspected revision now hashes to
`2f7a59480318d000cbfdd948fa61640209ef27ad2127445cf5f099f27e32e7a2`
and includes the STORY-2537 unpushed-seat guard. Its original snapshot hash stays
in the table for historical comparison. This is not a fresh exhaustive snapshot
of the changing host directory.

## Disposition rules

- **keep**: explicit existing diagnostic/operator jobs remain under the current
  script-deputy contract. This does not schedule or authorize rerunning them.
- **move-to-aida**: replace control/observation logic with the typed CLI and
  supervised state machine described in the sketch, then retire the wrapper.
- **move-to-deputy**: implement a fixed host operation, preserving existing
  attribution and gates, then retire the shell implementation.
- **retire**: stop generating/calling this wrapper after callers migrate and
  active work drains. Retain historical job evidence under normal retention.

## Migration groups and external edges

`fill-slots.sh`, `fill-slots2.sh`, `fill-slots3.sh`, `fill-slots-wrap.sh`,
`queue-custom.sh`, `launch-*.sh`, `chain*.sh`, `resume-2708.sh` and
`after-3493.sh` become dispatch/queue requests, including per-request vendor and
explicit resume intent. Do not retain the scripts' force-claim and cleanup
fallbacks. Generated launch scripts may contain special prompts/options; migrate
only supported typed inputs and surface exceptions for review.

`auto-train.sh`, `train-run.sh`, generated `train-*.job.sh` and `ship-chain2.sh`
move to canonical train/ship decisions. Explicit `ship-*.job.sh` operator jobs
remain historical, individually authorized acts; they grant no future blanket
human-clear authority. `auto-rework.sh` uses typed verdict/CI facts and dispatch;
`auto-publish.sh` uses the existing native publication lifecycle.

`launch-review-watch.sh` and the later `review-watch-loop.sh` move to the
supervised review-watch service (AC2), the first implementation slice named in
the signoff. BUG-2471 (Completed, PR #3536) fixed the snapshot-era behavior where
a single PR refusal exited the watcher. Build on its per-PR refusal handling;
the service must retain bounded retry/backoff and reservation-preserving restart.

`host-merge.sh` becomes the proposed `aida pr host-merge` local fallback after
GitHub's server-side update refuses. Reuse existing rebase/update helpers and
preserve its STORY-2537 guard refusing seats with unpushed commits.
`push-held.sh` is retired without an equivalent automatic override. `sweep.sh`, `end-finished.sh`,
`seat-safe-to-end.sh` and `unblock-reviews.sh` must use canonical hygiene and
review admission rules rather than inferred process death or log matching.
`stall-check.sh` retires as superseded by Completed STORY-2476 (pipeline stall alarm).

External dependencies include `~/bin/aida-deputy-shell` (named runners 2 and 3),
ordinary AIDA commands, `gh`, host Git, local session records, headless logs,
`pgrep` and `/proc` scans. The existing script deputy remains an operator-started
service. BUG-2468 addresses legacy runner compatibility; this migration must not
mistake a runner.pid-only or namespace-invisible runner for a dead deputy.
Adjacent `ops/advisor-waiters/`, `ops/*.sh`, backups and non-shell files are not
part of the requested `advisor-scripts/*.sh` snapshot; inventory external callers
there before removing a wrapper. Recurring review-watch launches and the
script-deputy boundary were inspected alongside the scripts.

## Per-file snapshot

Original snapshot: 267 files; revised dispositions: keep: 9, move-to-aida: 18,
move-to-deputy: 1, retire: 239. Including the two addendum files: 269 files;
keep: 9, move-to-aida: 19, move-to-deputy: 1, retire: 240.

### Later arrivals

| Script | SHA-256 | Disposition | Replacement / rationale |
| --- | --- | --- | --- |
| `launch-BUG-2469.sh` | `a8eb0c5899d507b681c6592dda5d1b8cfda7a87cc9c5cd4af8f0a85e4fbbcebd` | retire | Generated launch artifact; replace with typed dispatch request/receipt. |
| `review-watch-loop.sh` | `042707e6e70a7849258704f38ea8ed7dc9bd98245183de76c63dfba2ff0c6cc0` | move-to-aida | AC2 supervised review-watch survives per-PR refusals with bounded retries and preserved reservations. |

### Original snapshot with revised dispositions

| Script | SHA-256 | Disposition | Replacement / rationale |
| --- | --- | --- | --- |
| `after-3493.sh` | `febe1b22e76590a2f6f199f0a5e189891d774dd552b452abe8498e9366fbe550` | move-to-aida | Dependency condition in dispatch queue replaces polling waiter. |
| `auto-publish.sh` | `eb68243810f78e2af8ed52f619b8baae40363da24ef2e89db7ea6efbae7d0dc2` | move-to-aida | Existing native host publication lifecycle; preserve punt/hold and Refs. |
| `auto-rework.sh` | `747c0385308b59a3431cfa7f13192e8540c96b2d1f9fc1ca34c586a0ed1c6fdd` | move-to-aida | Supervised exact-head rework tick; durable deduplication. |
| `auto-train.sh` | `044bc15e4cbf64366df357064c5abd21b55fbafb23599248a25174b148207321` | move-to-aida | Supervised train tick; ordinary train and hold gates. |
| `chain33.sh` | `d882edc54e82352a738225e269dd6b61cdeed1f8cd21292339099c8cca0e9c89` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain36.sh` | `7f73de5bb662b9fc6af5eb12c05d774f18c59393cd692fe33629909d7777b0a2` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain39.sh` | `bdfb1eb45cf2010c960b96d1c766abb17057aeddb246abf57431a3745580bf9e` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain44.sh` | `6b16d38814853dfc431cff022920a0b8d154d55e5f535f775dbe62904a82fb63` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain45.sh` | `0823bcf4006f8fc15cc889ed0e388e440d623470fff6df271fc7b514b443906a` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain46.sh` | `897fa5aba1da8e7b8bce58872a3c127e7f3487a1f0335e870df36c999cdb9a68` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain47.sh` | `dffd9c8c1c187acfa8fcbcc32393d6706cccaee3161f357ac4d62701b22ad840` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain48.sh` | `f426af74ec611da043bbc9d1df3fda6afddf4820ca971f8791b0e312ca8e0c90` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain49.sh` | `fffca9d5a0a987075553f83eed8850ddb969f573d9e1412ce25fd87a091f64bf` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain50.sh` | `9f7a080034c07cfcec7667cb7e7929d3ce3b30ee050379fe087bdbf59b5b4027` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain51.sh` | `b0dac86aeed334c7c1dc6a7a776e7f72c1b5b04af2f969e0d84507584bdc2d8d` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain52.sh` | `251723c89e4db77fbc43a95fba5ef56a9a3cf7e44efc2107764fba855c9572e5` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain53.sh` | `feb9281f84dc30ccedd6bf7e1121ff9a30709ef4a14c8585a57780d5a069242b` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain54.sh` | `9565fbe193f235c8ab0c9abe8e624ecbaf166d7d335c77262b286d5b68efef9a` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain55.sh` | `16b828476a519c015d45f1a664cd24833c73e08bee2b1340670c628c62becc35` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain56.sh` | `3d0e98185d711b7446198fd3515a29e032f6a74cf05dc6197f960591c20523ab` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain57.sh` | `15af7309b7aaba132d3e5e52410afd4c3e1486b4c840dad12429be56b3ad7e3b` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain58.sh` | `bdac3ccfb6ea4ae038e0c9a0278b2d7fe1c8cc26b160a5cbd1c030eea16b0bd6` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain59.sh` | `58bc93eb33a97039c5a2f70231a18bb62fd4620b541b4e424d4c3a7359bb4fce` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain60.sh` | `6720338edb10d9858b654e660afb55286530f7be7b024ab743093e0654129fae` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain61.sh` | `d0f90ea6bc8996309b9e60c704eb9e8fd70ea075cc2e08cb974c303441f1dc4e` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain62.sh` | `d591d3e564c5d8c3e35758b877809955e1eb1251331174c8f03ef7aa6d78ab8f` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain63.sh` | `17455be075459332e9fbc59704eaa27555eb2c649c3dd1026220aa2a07df3c69` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain64.sh` | `fb453e51613c4bccaae0e1002dfbfea9fc4a6b6f255079fa81e0532bcb530d8e` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain65.sh` | `6ff2bb72cb2fcfe846d357ba9498c0c26bcd0fe3cd0aa9c60d6a42bc230f3be1` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain66.sh` | `186ab14a840e7b83fee7a4bded55688257492d325cba24e908446b979f777904` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain67.sh` | `41e064c4affba4f9284ca8cea4a31385304634e7f4b8f83c1224f1e7ef2f7981` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain68.sh` | `d0cb53e15293db4a86d785608a297b98f01aa42cb1631486196f7bc60930a6fa` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain69.sh` | `8ce074d9413577ad98479bbee07a1d5cc40dd9ec42a2e20be3226b1e0f0c216d` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain70.sh` | `5467d908e1f44b730ba462bde4fc6cfec6f80dcf79aee30eb49f992756edc188` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain71.sh` | `ed2cf7ed4c87bfedcb2c808b237a53f9f5afc9d2684e67a47ea5747d016fcf19` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain72.sh` | `0cab0cab417ac1d2e195521dfb94c3c8055e1f63e29d5ac5c72b834bab86be5b` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain73.sh` | `247a46a3eb2535021331dd7fc173066dd822c18fbc52f0ecc20dc88975b5f39c` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain74.sh` | `61c79779ff84372312b62390bdf89ce5e71ce6bc9f9d8b32598278dca8acee00` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain75.sh` | `291b9ba737be609fb5a58dcb40a4bec016da8e3df9963f660b11667e18dbacf8` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `chain76.sh` | `88fe3424a64da03ff8a933b615bd6949c4d090eaf49cf0d73fe29c8e63412f5e` | retire | One-off spec sequence; replace with queue requests and conditions. |
| `compete-TASK-1715.sh` | `a3ae397a0d39bc44e9839e76a1c0dc9827d475f10973c08fd5dbb3c005c14b04` | retire | One-off wrapper; use existing compete command with its gates. |
| `conformance-agy-3426.job.sh` | `c272dbe8c5506a261490bdd80bfd9c7dc9cb1eab2faf23aa27602362acf35b22` | keep | Explicit bounded diagnostic/evidence job under existing script-deputy contract. |
| `conformance-agy-3426b.job.sh` | `ca9aed8a4323df48344a9a92f678d5686b6a2f5fe4ba2ac025ac9bd7431b83fe` | keep | Explicit bounded diagnostic/evidence job under existing script-deputy contract. |
| `conformance-aida-boundary-rerun.job.sh` | `e65813a703302fa89fe5b32439ad1d534598720182da0cd8f8101f9a7b52f6b8` | keep | Explicit bounded diagnostic/evidence job under existing script-deputy contract. |
| `conformance-aida-boundary.job.sh` | `a015dacf10b69b88b83802bf2c633bdc1a1806489a9df222bdf6e15eb26d0f83` | keep | Explicit bounded diagnostic/evidence job under existing script-deputy contract. |
| `conformance-diag.job.sh` | `86057aed04088b22d88d2c0da3e480bfd34296c1d253bfbfd5c17f43287c34d9` | keep | Explicit bounded diagnostic/evidence job under existing script-deputy contract. |
| `conformance-fresh.job.sh` | `72ae5ce77733f60ea3fc29b02b4adf9a3648540a59d118ee47cb686f7eae0103` | keep | Explicit bounded diagnostic/evidence job under existing script-deputy contract. |
| `end-finished.sh` | `399a0ac670275b61e829be19de575a8adcefce2d12fafcf574b821f13d45348a` | move-to-aida | Canonical session hygiene; retain unknown/live/dirty work. |
| `fill-slots-wrap.sh` | `2c80ca65dd0c7266de7ebac9d467c930a156aa65ee5f5f767c716498adbfb5fc` | move-to-aida | Replace with durable capped dispatch; retire wrapper after migration. |
| `fill-slots.sh` | `efc39fb4dd2c2ba2b49dedc061a0ecf1ec66ab156b19765d5f6842e5ab4cb982` | move-to-aida | Replace with durable capped dispatch; retire wrapper after migration. |
| `fill-slots2.sh` | `2394b6cbaafd35ac8f44c50b8fd83766c88e0d89e65126380b51e270deea8661` | move-to-aida | Replace with durable capped dispatch; retire wrapper after migration. |
| `fill-slots3.sh` | `391a1c3c70106cb9ff17cbae9c7527752d3c17ccc05697f6ab1523b8e27950d4` | move-to-aida | Replace with durable capped dispatch; retire wrapper after migration. |
| `host-merge.sh` | `f408800d4374b59acd683c925bfe5e1f6e16e0c3d35b7ef5b920e6995499731c` | move-to-deputy | Local host-merge fallback; reuse update helpers, retain unpushed-seat guard and conflict receipts. |
| `launch-BUG-2194.sh` | `db2c2618b09d1935965348e817a5f76f2ac83226a4b6fbb16df00804513d1ba0` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2390.sh` | `6f0e55c095af8c526f49f8ffb6f880dac6705a0b9c870ec40a1de34703548e5f` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2425.sh` | `b0a274d7a3eb59a6f9b20831b7f05254a6327df868ad3c95fb136ff5ae5702d5` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2433.sh` | `020e146394d6d45b90894a5d309366949f6b8be187c3f874509399b5858a1b8d` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2434.sh` | `f5c78607c99c28b028ba4771a7ed28df1f18717e8be8a4f8cedf77e2fee8b0e5` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2435.sh` | `e0b5dbf7bfe00229a3e77d31a402b04c3ad256db4def8de3b84ff61830a7e638` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2436.sh` | `17bdd7c21f6811dcea1b4807581ebcc124cd3b29b255e07a8a74aa499f3f4fe7` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2437.sh` | `5430800f271bba4882f06d0b629cd98e873688859d4f09e1f569b33c1f2c54ae` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2438.sh` | `205c9b7db55eb51ec5d2a51e9ca21ba0878566303d0f979598b9cdabe17b4604` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2439.sh` | `48f39dfa8d0aed0d64d05556756bd665a4c8be742425cad303f87415414ab878` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2440.sh` | `b07bd15b30d7e40a5687ff765785bda609c14f9d3a2825895851bd53e01e196c` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2441.sh` | `1c20a230a9d841c22192654a1eced4e6317c31a4798d12a163e43ae1c5a09942` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2442.sh` | `ef2b8423c4dbe6cd67ecb71dfeaf17b733a04737aa1d2ab109f6c73d4ba62961` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2446.sh` | `203b1c3c3d8586c9f4667db33b6e62fadef569a9b85dc748c850a8b0213b44ea` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2447.sh` | `9eb5e5b8a87d4fa79725705785bf66706266492006be8fcf08d79d31dbbaa7c1` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2448.sh` | `a3bcede70b3eb3529254d188b7afef7ed71cf7f0b86284ba98030bf9b6f76d79` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2449.sh` | `323b57013ad2abf8dd0833609b2d78b9b28e234260e29948e12a55cd7ebb479a` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2450.sh` | `7c7a1a93d11af98b8c0cebcd69f3c67cd7218135b5b174cee2de50bf4bdd8f4c` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2451.sh` | `0fe483d335a6b99f5cbe36dcff2bcd51167e0be9b4ef83326b2227a62e8dea62` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2452.sh` | `e833681301d3a94a16f68099fb01bfaf0126b2e071a3b3eeedf9527ca43d60f5` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2454.sh` | `21bd63631c6be7745fe3adbedf8093d732f64ac1f76f2495b2ecd53dfeb913ed` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2455.sh` | `66d260b70b386e52429d75037cd3d7de0671b3ee64fd071ca006b1d44f559b88` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2456.sh` | `41c814d013b718496bb2dd3e823ca6690df8e5c76a8d0264fd41d7ca41cd73e5` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2457.sh` | `adc1e1f2131985292ada61b75f9288bf2a487fcb005fb9f2ff4f7aa4f10f577b` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2458.sh` | `b621797cfbc3fe282d1988a0d986356978c896bb384c58a219ebeb24216f62c4` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2459.sh` | `b923dedcb75b9b558e1559d7feb8bfa9fe6682839d84b2fd4ce0eba25545741e` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2460.sh` | `2022e697cbc7714ab0fdee1cbc05aedb53774a3846508b3a02f675de0487b432` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2461.sh` | `e3482156e11152119f18a203cc23a4fb1e7489e25c6c25ed820b0b0fcd720a55` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2462.sh` | `bd64f0ae2dbe28a84aa0c562299e44486fa2ef567666b1a468c990fb6808ebb9` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2463.sh` | `4a993f62e2211e460d2146e98689683c4db04382c36747f4bb4a3354e3b0f24f` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2464.sh` | `d552689920d7d3f6d350d1094b88bc2a33936113c56050833f28c280abc1000e` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2465.sh` | `86be02f0fb4ce0f9b4102c5af3c1aa9421f3c0bec7ec344c7d1a10df7eaa549e` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2466.sh` | `5b8007c434d1fe5395a4f0fc922b1b3d4567bc858d6f76baf19ed42d3319f44e` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2467.sh` | `1c7cd5601ac1a7def8647d194acb334bac9f3bfdc7134c29b166d27fa49f6ddd` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2468.sh` | `d7f42fba41c3a6d5333743a4cbfc828fbeb66490b9e0e04576254a6c99214421` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-BUG-2479.sh` | `c366a2a1e3d58a52d7c686fc5c8bfc88908effd1edd3c184a074a9f4c5204fc6` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-SPEC-447.sh` | `14799fbb1888a9ed629152f16595cad08e8d485350f5405c163eb94d81359551` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-STORY-1468.sh` | `d07276f56d1960a285b997598bb47f11621682f60ca4b4de21de8db0d846e543` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-STORY-1651.sh` | `759cf061eaf597017268fbd0e1e846ffbc954dfa7891584e76290d2c6e9ba103` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-STORY-2324.sh` | `6949df99362071bcf71ca726238e1bc34c319de99b0cd3ec5f13e437e329dcc1` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-STORY-2326.sh` | `b9b75e1171cc72d3850c6b35c949267e96cddfa60e466885ee888d58119226d2` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-STORY-2331.sh` | `3a2e1bf625beb9c0a44c4ffb801f9e77d19c1fb507a733dd116107719bb1ae0a` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-STORY-2338.sh` | `3e4eb0b606c24590f71f19609aac98e8d4013d12ba982f8053fab3880d0201aa` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-STORY-2343.sh` | `706d54d03d10ce54dc15324ec771b698d31d2daab42936812dcd6c1b57f20d7a` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-STORY-2351.sh` | `0088a497ece44ed49fae062c4684a85969f2e592bc0e86bd24f62b5c8e08e245` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-STORY-2401.sh` | `6ca00f477ac0470c95c704210448c8e981c89a9dc8db2dc41b80731cbe9910b5` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-STORY-2402.sh` | `2cf533f2ebc85059f28c93499482e229999eadd72580a86c8350f4d26b7b97de` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-STORY-2460.sh` | `cc46648bc68cd6674dd82414d392cb0e1ea96531fd518f0c050f92e4dbfc6f83` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-STORY-2475.sh` | `209cd63fc8cf8dc3dee26e756d862610cabba05df4bfd19a06554fb60759f8b5` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-STORY-2476.sh` | `05f0a29769bfafee44099fb4ded6e926920a256cb34196b7b124061feec16ed9` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-STORY-2479.sh` | `3773ad41d9abcced2e7951564d79623e0aed0f9482ef521ba45b273e547ace93` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-STORY-2480.sh` | `cc6ed5391707d94bcc91ac08011f1b3b8a7b07748448472c1127872955e097ea` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-STORY-2492.sh` | `2cfe9d42fd4c8fe9bd49de4008c930cffe316a882ccd2b4dedeb2d61b4b5172d` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-STORY-2537.sh` | `7d9314c96e87bc3108df2e562e6c9d5abdaced4def54a518927080f3b57bc168` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-STORY-2538.sh` | `942ec775c0690afa00081bfbf8bad57b606f3bbe690e1d7ff9a1bb4b2eb36dfc` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-STORY-2539.sh` | `4ed58ebbfaec623a6dc3a03248af096556f5c0c88c2197edf6a00ca361ca2e4a` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-STORY-2571.sh` | `216a3833cc181912c16a669accb86402da813b60d1c6c40d69f9ac5bccc4e599` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-1059.sh` | `7544cb6a8a9dd3fbf4866eeed595b4babc12e65af79e1229a7b1b884ba147ca3` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-1299.sh` | `87b70e7befddd62e1d92b62cdca3be9721e77bd5810c7b5480a7d13d32aeb491` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-1471.sh` | `19a590d2779933db4571d3daea64e973d46c20539366afe73d38904679c3e065` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-1472.sh` | `721426da2678acaab0be4c1d0f88bac6866ab08e6d5b2b89439021f24e95317f` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-1595.sh` | `d5016cde687c8fb3946071a446f2339a1a52470a07252fa73b0f8c39267cb552` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-1715.sh` | `6df22dc3bde3471d418c676081a6d5da75f0eee9e91c4a21200a388f2ca15eaf` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-1939.sh` | `7241030f3085a3252901dd6731200f6cb0dbcf1fa7f4d1e2cb86d6737b7ed456` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-1954.sh` | `6227520200d67af910ae8ceb9cd3a85d364441e267a3d37bd8a7df23988bf82a` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2000.sh` | `c4d0b439c7f05854be1e4813f10bdafa0c34cbdfecfd24263e324f363262149c` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2020.sh` | `7ce2eaa6fc89089d83cf6ce75559a6528f18f83f6a99b2565bddb0f3eb5a696a` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2022.sh` | `a84edadd914a21af68246dc5bf9b088f06e3d63e0fb696acc4a22075345571f8` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2032.sh` | `84d3449da32ae3a4f705be15be969dda74a33a372941987b57be42a2bdf3a6c6` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2033.sh` | `3a6da7624ed630d29dad3546a1eb2a7db0a6e539d61b0367b47ef3b598a3dfb7` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2688.sh` | `0325a381ccde333ff89ccde9c2974d758aafe1182f320bfbc799f084e9553033` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2689.sh` | `3079dacaa9e3f70bb00cacf4476d47cbe8f9d2d5c0e6d5681974ea74abc4c2fa` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2690.sh` | `af75531560a7b5e84bbcd3fbeb66cca32acada5d5a80bd2a882525783039de27` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2708.sh` | `c6ef4399ea12d048cfe721dd5ae6fc6f9913532b939d211b8a3491a539577361` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2712.sh` | `e9f779399dd0b985c3b47f1795b116fa771ab5e8b9839e7f59eee0f8b54c319a` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2793.sh` | `ec52844e93d89c51cc75d1de2036a95ca8a9b7ab7e910348c27072cdda18f884` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2802.sh` | `3d14af3ba9770301cd8b3000f02357b111f3edf9fe55c9579710468e137b96df` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2805.sh` | `06504fc3fb9baa20623e86536624adb0c611da705ee4761999f71c8ef90a54ea` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2807.sh` | `c474980dc95ec1bf49c8c59a68849b34ea21f327675e5585388c1752367492ca` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2813.sh` | `58b77024afb71adcd2c06e69ace50c345264e85bcc719d6504ccb5852472e273` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2823.sh` | `fa99b9a903d0c5a52339f49c8b9c8859c1864fe6bbedc1fe5c0241a19978f9b3` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2829.sh` | `cfb61107d0d3018991042a452ae0fbb2b5891dfeab1fea9cdd4188ea0087e794` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2830.sh` | `9472944147746f5877a55ae57dadb1e07a6b24c9478eaa8429604574f7f05289` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2831.sh` | `bdeb0f833acec0b3852086a55abe770779f50b22644948236127432f356d09cc` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2832.sh` | `666b9f355f7730364bb5cf35fc11df012bf89e08153c89e790308940fc679e55` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2836.sh` | `5a56614b06112e927f5ce9cde75cbccbacfa137be3b114fada6ddfa9c020e3c2` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2838.sh` | `4d36b97d53bff0d79d5679502c94f3dd7718e05ff56394cf980d71187301cfff` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2841.sh` | `b07c32377b86b4822ded4530478227c3e7422a64474a8324e51c0be5f5508899` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2842.sh` | `01e859b574594445c50c98c5564c057664710e1314afd6fa1aff83b831d6e31e` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2852.sh` | `c0807ee77fceb24dd816397669e58797537f6bf0e7fbad32286e15b48eab2d5b` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2860.sh` | `140929164bc7b48c4e1d35cb2fc59fea3dfeab6f92ec7920dd70e13e3309e071` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2861.sh` | `1703c8d41ac7716da3936d0687eac8409fa8dfbc17039243e542533ae42d5137` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2879.sh` | `8522cb1073c5028b6728a5df461ebaf78f647c0fb87697af16fb495c092f1368` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2886.sh` | `3e8dad1d2940545c9e7e2ca2929c57fbd2b6d60eb17a3ccf03885e10591b309c` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2887.sh` | `9326ed192fc6b9c8fd3e20cd5f595b0d79a36040f8bfc5df8041c3cd0011427b` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2889.sh` | `09177116c78fcfbbbf51a5765d10d70cd10b62b40fcadbf4f7b27dd26345d352` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2891.sh` | `1d9f541a98991638ce6bfb86d1b0e284f42acb3efca2e37ba956fae0dc8428a8` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2902.sh` | `2f67546ebc6ad6fc4e420827752792a932588df354785ad24d3d4b0bb0f45fd3` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2903.sh` | `e81b0272dd5c72a45ddface6d84aafdc05cc7b4f00a0d57cb85cfceda85a8faf` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2904.sh` | `c5dad2b800ea12966ea314785c88975a44580042da4d999ea21b7ecd58744cf4` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2938.sh` | `3519aab7de86cfdcd21000caa27ba42001e2d4586b12e5fd304f10fae75196be` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-2995.sh` | `43fe1ae93d6f08f8e3c43ec08bda1b0a2bec127a1a97460d84128323bba79933` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-3046.sh` | `84b9452b70c7f6789541920cf75e848874cf1b6024c37c9b15affa928c57d6a6` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-3047.sh` | `fa386a61b8ee1b0a892a6b6ec3a6020a53dd7d5e452bf3e7d8499268a13b68c2` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-3048.sh` | `fc135063095c5be4af2a683f317ec9e15202c860d87b8acfbb6954e90f74a1ab` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-3049.sh` | `fef2e2c404f7cdc3f892a3fd4871c8d0b5aa8310606944fdc3e6b4d4a383429c` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-3072.sh` | `7d562708ae6ecbab68e2db6243fe6a2522d8ca9dec696e8a9e2b4cf3031b44af` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-3073.sh` | `e72cacac6ecc0bea4349be96c81cef5e3566a4a9507b7e7d055d8cacefbdfcee` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-3074.sh` | `b031839096fd1f02b4e8a8e9c402f6187fad5a430c2d1d3e9c819a817e501b0b` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-3075.sh` | `c2783ce8c9c53d664845637ec461eb9cf52f2ab1f30f989f4ed21b86143c8257` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-3096.sh` | `c2495edd4a446fd54353ab6c93cdb9af9f27b0257ba42fbb6a3bda945e720550` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-3110.sh` | `2b5c560a642d21f2bdd3e3ead00dff41ca6c98864b85a12c39d39cab4e821620` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-3111.sh` | `f25a93062afb5aaa3cb08173308e3b935e4d1acb9bdb9ca94f9cde330dbd48bb` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-3112.sh` | `7d6137afe3a98499b5635480eb4f03a2d05180b035e5aa7ec4910a4e1524f5be` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-3114.sh` | `b2c79cda7baeaa4a8ed1c231436d816c8b8d4d0b8f2a56063a5e6bcb7bdba552` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-3131.sh` | `4bb2d883d1b4f4175b227310c0c9e9796cffdbcc8f4ca9c89bb2a05693efaf55` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-3194.sh` | `803f2f11ee74861d3edd168a2169b2ec87e3c1ea5fb08c35f5c57b43f79c519b` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-3196.sh` | `4f9c87fde00425777231381846df7707ca37581fe62f6ceda3093ddaa8d7a6bb` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-3219.sh` | `2fdaac831a57862616505788cda0fe6a24eae3d5c01e1090da0e041ba05375af` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-3222.sh` | `17b2933b41118eb88aef3a8acadb6c0e64118f41cfc04762a6a6800403c51083` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-3225.sh` | `01d3d15b551812509bccffdb3db9cb270a6c8d8693af16b4fbbdc637b856b97d` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-HOME-868.sh` | `1c34818b50484514f8978a092d8b624e6c358b4069859e5dcc2b5959afa47d2e` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-TASK-HOME-872.sh` | `a6f67597a4c652a9399fafb4806891d78d563b41b060453a0df54c5c5ef44139` | retire | Generated launch artifact; replace with typed dispatch receipt. |
| `launch-review-watch.sh` | `5566f0432cd633b2c56fa6aea224ede0bd952c4ad83b5a15066dfd53c11c7f46` | move-to-aida | Supervised review-watch binding and rotation. |
| `push-held.sh` | `67789050b3670c980523b1ef25df650a4ae39b352ba1b77572cfc756f33edfd0` | retire | Do not reproduce automatic publication of author-held work. |
| `queue-custom.sh` | `a9cf4c70f1dd115855ad6b65d86fd48d221d90d1877ac5ba74d0d2c503997da8` | move-to-aida | Typed dispatch requests; no arbitrary launch-script execution. |
| `ready-to-ship.sh` | `32731ed0db53ca8200fa33f653f3dd8f347c7fe4fc95940014c7c61113499ee2` | move-to-aida | Typed readiness observation; ordinary ship remains authoritative. |
| `resume-2708.sh` | `b53288b32d7d86caa7a71cd243e30c5aedd10e0b16b0d17479e6ac55dfed0cec` | move-to-aida | Typed explicit resume request through dispatch. |
| `retention-activate.job.sh` | `8e3215698e00dd2eedf2f087867581168165a32a8a94187da711cf32fdeec0c5` | keep | Existing explicit operator activation job; not an automatic service. |
| `seat-safe-to-end.sh` | `b88042fb2164f6b8ad3caccda4520e3eb57c31b345be05abd70f3a9af250a0ed` | move-to-aida | Canonical cleanup checks; no shell-derived death proof. |
| `ship-3351.job.sh` | `98bd52b5b3c23262463f4eb6c6e7c2d870a49d0933e840ac86c2a6a15845ff4f` | keep | Explicit operator ship job; ordinary human-only gates remain. |
| `ship-3485.job.sh` | `290c604bfc0f99375b265426187f61fc5416fd8aa9b2556b320d6a43330d3067` | keep | Explicit operator ship job; ordinary human-only gates remain. |
| `ship-chain2.sh` | `e1f42809438bf8d26d4490f3bf8047439bf516c5118215d477166a4b11163d6c` | move-to-aida | Typed train/ship lifecycle; retain all ordinary clearance gates. |
| `stall-check.sh` | `8f2d8da6b18338629b1a4a3004c4232378fa28d2ef7b5d150088132fd3274231` | retire | Superseded by Completed STORY-2476 pipeline stall alarm. |
| `sweep.sh` | `a3995067da403ff43c58682008b96ae8b43390206519b44a3dc244a9ab48fd8e` | move-to-aida | Canonical hygiene and review recovery; no automatic claim release. |
| `train-001826.job.sh` | `280d32bbccbc4df9811375ed7e933c4481a290770cbae30f1bd91dea2f315103` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-002146.job.sh` | `15bf003794322e67def8d6d7011ca2a8325f424517cf9459e4b09f4010c003a9` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-005616.job.sh` | `b16fa70a9a42abd2051a1cb05e33ff7194eeb0a24cb33039aad8cc1692e92926` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-010102.job.sh` | `f34c4e9aeeba970d32bfa2a586d4e35173a6b75fa98199fbe02af63e189eeb92` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-010134.job.sh` | `6f65ebc1fadc2bf7c9974716517e82013d1ada77526b9c81e97dbd546189ad94` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-011758.job.sh` | `d0385dd71dbb717508b35dbcd4efc4ff9c93f6736639b0e8c4d7abd6cda45be4` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-013552.job.sh` | `f1bae52b9395140121f11e091a261075d045f526da1918f96d5c63f6ce4d9ba0` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-014023.job.sh` | `fc2756e1c3edcd966096bee6914942a617d912722b8328c76ec61274c19f659d` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-021325.job.sh` | `2d7c9b7e5044cf64600eeb05bdee47d6d624fb8d1f4a885c765f63fd49977b1a` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-022609.job.sh` | `415c3c915f5311788822f8fb482f6b922c27b2035a9854a5cedcf2350f27ee31` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-023057.job.sh` | `4513b3544a02e72bd987cf36a9f09ccef5f31ac1ac78bbf6cd43f75e4e67e3c1` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-023520.job.sh` | `592871024989f90108108fe9ca614fc59de09bddb98568735429678072063a05` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-024515.job.sh` | `0d7c3ad1586e87821bef43ed66ba96016872284564dd8afa91835fa73c6ecd6d` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-024915.job.sh` | `350b9ed49457e857c143a5aa36b2d89f91d50a97a0ac0ac8b7185822602a2d26` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-030803.job.sh` | `25a2d4bb0d5fe85112d77c3ffffefe5896a7faefa895243abfa5dd5e537c9f8b` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-031355.job.sh` | `85a8e24decb3c7dfe9becc4d406da78fb8471730a276eb5529981f81811ac1d4` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-035030.job.sh` | `0725e328e281ee12a4ef963b44b0d52b289106a937f94505ab4affe321768299` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-035059.job.sh` | `eb11f00e1389a60fc9d461d8d315f281821126cd6fad8d8989ecb0eb02e88b78` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-042319.job.sh` | `7e4d34354dc8df261192e7ee59ed4ab7ccc97eb22cb5dbeb3467f0ff289d2ce6` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-043545.job.sh` | `78daf8906867705b99f48b65d805e9b98af44d4a86393a31e68cc03a0683f8f1` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-043755.job.sh` | `ad1ab6d39f8feb8a457c70d3fd77633930de7962756c51d6287ab5e82f3ad165` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-045254.job.sh` | `b9262205bd95698ab211a319c99a76d4cdb13eabe94d0ff58f813c52b237b099` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-045555.job.sh` | `88c7646d97ede25d84dc04aeac2ba134895f8bbf64f1119ceeee17cbf1880ebe` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-045556.job.sh` | `c18bb790a8861b207f6897ecfa28d4c20b50fcfb13b2333393fef300e84fe8d8` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-045907.job.sh` | `42a5092033751024f47fe1345ec0b6dc91ba5946763043a6240221927ac568da` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-050333.job.sh` | `2943461c2a9a670a4ff0285eb89b3e4e8ce078bfd657384f8840515b29818f73` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-052802.job.sh` | `fcfc8cba389cd943444f46020d12d3e523ba335778daa51a9a4aa83cedbdb965` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-054528.job.sh` | `911d11b82c7551482c4d0bf5db36e14ab3137e9b92503a3096d6be150b0535b0` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-054600.job.sh` | `7d33c49e58099f08665dde74ec0ecda7a3b3631200e34a3b6d2e1770ab13ea71` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-055255.job.sh` | `df64f5dcaab140657623ae0127262ef408d7bd5b2474052054a3c458ec8cc820` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-062600.job.sh` | `642480869c47f4428504257c13b32beeaaa7b6665a9a7f590aa71ce84c5492d4` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-065841.job.sh` | `042bca7210972b6781175c349a46b2293eaf40c5a319b3c4a18dd6cec6d62377` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-080218.job.sh` | `90e5041b4d339d3e861a7f034cc78f601547a51ae0a0d2979dd4180f1e94fa9c` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-083354.job.sh` | `bbae76c44359a9cc1663cb1281e1ccca19608c46c7a643f185dcefd444b5f032` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-084547.job.sh` | `f24ff75c7a93b790f13fb71e301bb0e07d46f84962d92069b0f12fa85b3c1a5f` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-090722.job.sh` | `3ce24557d01e7321aa1921fa95c1ac5e2898166686eac56762cbe1f2f95c0b52` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-091405.job.sh` | `9685c16aabd7fb9ce6e325b32e2479ca956eebe339bcef7a950b346c6ac44bb3` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-092558.job.sh` | `3537034f0c260bf8ae189864c3df68b5d536b7689950e2246e88c7d1cfe4966a` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-095745.job.sh` | `1ff676072686cfd3995c3219e345e44dad86513f9f04987cec0b61b092ef4d84` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-095908.job.sh` | `602533a8975e061d6b4c3673af9dfc4648d4f0a159d539161c042cda6df691a0` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-101201.job.sh` | `c8d411a09d5be0da2bedc93df8ffb78c020169f53abf1d81c6faab3e3ddd940c` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-103522.job.sh` | `df8f8835f3e074f1d1dba706c473fb395d22b3f111179a64a2ae4f2fc282eb2f` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-104638.job.sh` | `082f0e10dc2cfb32a2864918fa598158745eb57a3894ebf04aa7f7c723959844` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-104739.job.sh` | `70e6bf32569205c3dd97efeeb80781473f3cef6323a4edb9ad5ae0b691a1e077` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-111044.job.sh` | `563055706023fd3371296bee301e4d94e42a089d54e8a09fc66009138892ba09` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-112612.job.sh` | `4d8e7f73dc124a6d7f50cecb7c948bc4e4ca9f54d653dc8d1938002fdea56aae` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-113043.job.sh` | `f4ba18daf41904c861840a6943b5a9ead5d5859a35a5f93642c91f92122740c1` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-115912.job.sh` | `5470b7a5eb3e244e1e494c432f4f2848adad1afc9c20e59b1dc939e21f93742f` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-123146.job.sh` | `6f4cff67ae6dbf71bad4bcf0de09e1669f65929ca8bc7c12403970c6602851a0` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-124410.job.sh` | `7c10837fcec33422797e7554ce7cfadaa90ec8663073811d63079a0e306a2557` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-133238.job.sh` | `6576afaaba2a9701dc90454c2ec2049f97b6960e5d4565f500447178eda42b22` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-140541.job.sh` | `7a50066c12a18ad3eefcc1b45e0045a754baafd26a5492e6bbbcf0ceedc2ec11` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-141526.job.sh` | `04bee2a87126b0d460cdf13324989a9726b7d7cd9d9268c954dab1b004428b8f` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-150552.job.sh` | `712adcee341df24c714a09ec241d3f440c5a8e52e39f1432d2891bcaa63af47b` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-163829.job.sh` | `81d3a69db23daaff2329b11ce1cccd4e84251fccbfc57de4a5d163b5fe9c71a0` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-173158.job.sh` | `86546b59437cab4b149f3dddd1614604f0f5628bf0fea956dc6d1b072c8db65c` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-175248.job.sh` | `e300d856eb542e78522bdad1466c8862cae4c658fb06fb254db36e3677b7a5aa` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-180438.job.sh` | `326a1aa11a244d4f3c158cffea00369d54ee54eac889cdbaa2edc77a91d24674` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-183530.job.sh` | `a336ef97c12a0d69d245e84a50649e6d42985a5e724fede7558943632f025e59` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-185625.job.sh` | `439aa376a8d11ee55b732feda0bf44a4933cc9ff297a0b32ebca118f2de586cd` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-193825.job.sh` | `a47c9658688bbb98b3fb6a6a908d96f1d51b2b39221a77010cdb64aef5052904` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-194151.job.sh` | `5542f888625895bca8eed5ac47aaed7eb548676ec6559ccb599f319f82a71ebf` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-200255.job.sh` | `ada66abb17fb08474f9b9d7db6e673884a21046451864cd0d5a411e140020b7d` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-200938.job.sh` | `8e0a734041f20e4b3ce7afe59686d53cf18aa8603202f7fc6dacc8af34f2160a` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-203857.job.sh` | `53f60db95fa852d64d49fe245a9aafb15cca7794f526a7d4def2d45a54e8baaf` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-204740.job.sh` | `3b43ef63d7d22a6d8b66cec683101c536caba65a54704d15a4ccca5c1cfc5710` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-205305.job.sh` | `e0b9e5d6ce3acdb42bf286be8272e1aafa1c4d0037712048d6acd8fae99e00a2` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-210349.job.sh` | `4374458b236173764ead97c454379a3fa9b19f1b3efd6a96f6e2f31e0bcd3d77` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-211221.job.sh` | `de4056b6f9025b0c2c7ae73148d29fb7b86106cb21b28db1dd730975d147ec1a` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-213114.job.sh` | `8bd42bebe6d731db7205bbe8aa9904ddf78f3ab1fb64bbb11725400748a809cf` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-215135.job.sh` | `872733b3f4f3a9f57486b5afc209c6dc218f70f12092517311b7f4af541e5a86` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-220828.job.sh` | `6173506be0cf82b030ab380317a64a7c678b8c5d49035653b75ceeb77240bdd2` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-231445.job.sh` | `22c24fb135db9bd375516b1a77942791e5bb73747fa60cb452aae939cdefbb6e` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-232901.job.sh` | `4e62768c1777b104eaaf30f7681732f5fc32f2d83bdcd70527a7564b724e3358` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-234106.job.sh` | `5b211e2ae5b90ca40917a4486d3d8c8fc9a53740b17dcdae9b5d0c8c358906d1` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-234959.job.sh` | `bb571517360ba8ac2da1837044eed3c66f8f36d2f763def68ce9380451392124` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-235131.job.sh` | `ceebfb452c4f0138625122b4dff730b1c89bdf38e2afd419bdf48e1068ea1dbe` | retire | Generated train job; retain audit evidence, use supervised train. |
| `train-run.sh` | `5f59145275f743d213252b04425f34387eb2a81becf1b94a7de1c555ee6102a7` | move-to-aida | Typed train submission replaces generated TTY script. |
| `unblock-reviews.sh` | `80cf4d3fea8abedb1d69305eae083c5a0b36173cbc1a7d840fd45806009710a8` | move-to-aida | Review recovery through persisted admission evidence; ambiguous claims retained. |
