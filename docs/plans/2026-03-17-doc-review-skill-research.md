# Research: Documentation Quality Review Skill

**Date**: 2026-03-17
**Purpose**: Research best practices, tools, and techniques for building an exhaustive documentation quality review skill for AIDA/Claude Code.

## Status
In Progress

---

## 1. Documentation Linting Tools Surveyed

### 1.1 Vale (Prose Linter)

**URL**: https://vale.sh
**Language**: Go (cross-platform binary)
**What it is**: A syntax-aware, markup-aware prose linter that enforces *style consistency* across multiple authors. It is NOT a grammar checker -- it enforces organizational style guidelines.

**Key capabilities**:
- 11 built-in check types: existence, substitution, occurrence, repetition, consistency, conditional, capitalization, metric, spelling, sequence, script
- 4 action types for auto-fix: suggest, replace, remove, edit
- Markup-aware scoping: can target rules at headings, paragraphs, table cells, blockquotes, list items, alt text, etc. Can exclude code blocks.
- Supports Markdown, AsciiDoc, reStructuredText, HTML, DITA, XML
- NLP integration for semantic analysis (word counts, sentence structure)

**Available style packages** (official):
| Package | What it checks |
|---------|---------------|
| **Google** | 33 rules implementing Google Developer Documentation Style Guide (passive voice, first person, word list, headings, Oxford commas, Latin terms, slang, gender bias, date formats, etc.) |
| **Microsoft** | 41 rules implementing Microsoft Writing Style Guide (wordiness, accessibility, adverbs, contractions, sentence length, negative constructions, passive voice, gender bias, etc.) |
| **write-good** | Weasel words, passive voice, cliches (535!), wordy phrases, lexical illusions, "there is/are" constructions, starting with "so" |
| **proselint** | Archaisms, cliches, hedging, redundancy, jargon, corporate speak, sexism, LGBTQ+ terminology, uncomparables ("very unique"), pretentious language |
| **alex** | Inclusive language: gender-biased, ableist, racially insensitive, or otherwise inconsiderate language |
| **Joblint** | Hype/buzzwords (visionary, synergy, paradigm), competitive language (cutting-edge, best), bro culture, gendered language, legacy tech references |
| **Readability** | Flesch-Kincaid grade level (threshold: 8), Gunning Fog Index (threshold: 10), other readability metrics |

**Assessment**: Vale is the gold standard. Its YAML rule format is simple enough that we can replicate the *concept* of its checks within a Claude Code skill without requiring the binary. The rule taxonomy (existence, substitution, consistency, metric) is an excellent framework.

### 1.2 markdownlint

**URL**: https://github.com/DavidAnson/markdownlint
**Language**: JavaScript/Node.js
**What it is**: Structural markdown linter focused on formatting consistency.

**Key rules** (53+ rules, MD001-MD060):
- **Heading structure**: increment by one level (MD001), consistent style (MD003), no duplicate headings (MD024), single top-level heading (MD025), required heading structure (MD043)
- **Lists**: consistent markers (MD004), proper indentation (MD005/MD007), surrounded by blank lines (MD032)
- **Whitespace**: trailing spaces (MD009), no hard tabs (MD010), no multiple blank lines (MD012)
- **Code blocks**: language specified (MD040), surrounded by blank lines (MD031), consistent fence style (MD048)
- **Links**: no reversed syntax (MD011), no bare URLs (MD034), no empty links (MD042), valid fragments (MD051), descriptive link text (MD059)
- **Images**: alt text required (MD045)
- **Accessibility**: emphasis not used instead of headings (MD036)
- **Line length**: configurable max (MD013)
- **Tables**: consistent pipe style (MD055), correct column count (MD056)
- **Files**: end with single newline (MD047), first line should be heading (MD041)

**Assessment**: Complementary to Vale. markdownlint checks *structural* markdown quality while Vale checks *prose* quality. Both dimensions matter for documentation review.

### 1.3 remark-lint

**URL**: https://github.com/remarkjs/remark-lint
**Language**: JavaScript (unified/remark ecosystem)
**What it is**: ~70 rules for markdown style checking, operating on ASTs.

**Presets**:
- `remark-preset-lint-consistent` -- enforces internal consistency
- `remark-preset-lint-recommended` -- prevents mistakes
- `remark-preset-lint-markdown-style-guide` -- follows community style guide

**Assessment**: Similar scope to markdownlint but in the remark/unified ecosystem. Less relevant for our skill since we'll do the analysis with Claude rather than running a Node.js tool.

### 1.4 write-good

**URL**: https://github.com/btford/write-good
**Language**: JavaScript
**What it checks**:
- Passive voice detection
- Weasel words (24 terms: clearly, completely, extremely, obviously, quite, relatively, significantly, very, etc.)
- Lexical illusions (repeated consecutive words)
- "There is/are" weak constructions
- Sentences starting with "So"
- Weakening adverbs (really, very, extremely)
- Wordy phrases
- Cliches (535 entries)
- E-Prime violations (optional: flags all "to be" verbs)

**Assessment**: The weasel word and cliche lists are directly usable in our skill.

### 1.5 proselint

**URL**: https://github.com/amperser/proselint
**Language**: Python
**Sources**: Bryan Garner, David Foster Wallace, Steve Pinker, George Orwell, William Strunk
**What it checks**:
- Archaisms, cliches, malapropisms, mondegreens, nonwords, oxymorons
- Hedging and weasel words
- Mixed metaphors
- Redundancy (general and in acronyms, e.g., "ATM machine")
- Corporate speak, bureaucratic language, commercialese
- Uncomparables ("very unique", "most perfect")
- Pretentious language, Latin phrase overuse
- Social awareness (LGBTQ+ terms, sexism, cultural sensitivity)
- Excessive apologizing, narcissistic self-reference, metadiscourse

**Assessment**: Excellent source of anti-patterns. The "uncomparables" and "corporate speak" categories are particularly relevant for technical documentation.

### 1.6 alex

**URL**: https://alexjs.com
**Language**: JavaScript
**What it checks**: Gender-biased, ableist, racially insensitive, or religiously inconsiderate language.
**Examples**: "he" -> "they", "master/slave" -> "primary/replica", "cripple" -> "person with a limp"

**Assessment**: Important for inclusive language checking. The specific term list is well-curated.

### 1.7 textlint

**URL**: https://github.com/textlint/textlint
**Language**: JavaScript
**What it is**: ESLint-like pluggable framework for natural language. Ships with zero built-in rules; 100+ community rules available.

**Assessment**: The architecture (pluggable rules, auto-fix) is interesting but the actual rules come from community packages, making it less immediately useful as a reference.

### 1.8 Link Checkers

| Tool | Language | What it does |
|------|----------|-------------|
| **lychee** | Rust | Fast async link checker; Markdown, HTML, RST; GitHub Action available |
| **markdown-link-check** | JavaScript | Extract and validate links from markdown; npm/Docker/GitHub Action |
| **HTMLProofer** | Ruby | Validates links, images, titles, tag validity |

---

## 2. Technical Writing Style Guides

### 2.1 Google Developer Documentation Style Guide

**Key principles**:
- Conversational friendliness without frivolity
- Second person ("you") not first person ("we")
- Active voice over passive voice
- Present tense for timeless relevance
- Conditions before instructions
- Sentence-case headings
- Serial (Oxford) commas required
- Descriptive link text (never "click here")
- Code references in code font; UI elements bold
- Accessible, inclusive language for global audiences

**Words to avoid** (comprehensive list):
- Hype: "leverage", "robust", "cutting-edge", "state-of-the-art", "performant", "actionable"
- Simplicity claims: "easy", "easily", "simply", "just", "quickly"
- Vague time references: "currently", "presently", "new", "newer", "now", "eventually", "in the future"
- Marketing: "allows you to" (use "lets you"), "functionality", "best effort"
- Jargon: "agnostic" (use "platform-independent"), "anti-pattern"
- Exclusionary: "blacklist/whitelist", "master/slave", ableist terms
- Placeholder: "foo/bar/baz" (use meaningful names)

### 2.2 Microsoft Writing Style Guide

**Top 10 principles**:
1. Use bigger ideas, fewer words
2. Write like you speak
3. Project friendliness (use contractions)
4. Get to the point fast (front-load keywords)
5. Be brief (prune excess words)
6. Default to sentence-case capitalization
7. Skip periods on headings and short list items
8. Use Oxford commas
9. No spaces around dashes; one space after periods
10. Revise weak writing (eliminate "you can", "there is/are")

**Anti-patterns**: "If you're ready to purchase..." -> "Ready to buy? Contact us." The before/after examples demonstrate the level of concision expected.

### 2.3 Divio Documentation System (4 Types)

Documentation should be separated into four distinct types:
1. **Tutorials** -- learning-oriented (guided lessons)
2. **How-to guides** -- task-oriented (recipes for specific goals)
3. **Technical reference** -- information-oriented (API docs, specs)
4. **Explanation** -- understanding-oriented (discussion, background)

**Assessment**: A useful framework for evaluating whether documentation covers all necessary angles. Mixed-type documents are a common anti-pattern.

### 2.4 Write the Docs Community Principles

- Begin documenting before developing
- **ARID** (Acceptably Repetitive in Documentation): strict DRY doesn't work for docs
- **Skimmable**: descriptive headings, front-loaded key concepts
- **Exemplary**: include examples for common use cases
- **Consistent**: uniform language and formatting
- **Current**: outdated docs are worse than missing docs

---

## 3. Documentation Freshness / Staleness Detection

### 3.1 Signals of Stale Documentation

Based on research, these are the primary indicators of outdated documentation:

**Temporal signals**:
- Last-modified date significantly older than related code changes
- References to deprecated APIs, removed features, or old version numbers
- Date references in the past ("in 2023, we plan to...")
- "TODO" and "FIXME" comments left unresolved
- References to "upcoming" or "planned" features that have since shipped or been cancelled

**Content signals**:
- Code examples that no longer compile or run
- File paths or URLs that no longer exist (broken internal links)
- Screenshots that don't match current UI
- Configuration options that have been renamed or removed
- Import paths or package names that have changed

**Structural signals**:
- Empty sections or placeholder text ("TBD", "Coming soon")
- Incomplete lists or tables
- Orphaned documents not linked from anywhere
- Documents that reference deleted sibling documents

### 3.2 Detection Approaches

**Git-based freshness analysis**:
- Compare `git log` dates for documentation files vs. code files they describe
- Flag docs not updated when adjacent code changes significantly
- Track ratio of doc commits to code commits over time

**Cross-reference validation**:
- Extract code references (function names, class names, file paths) from docs and verify they still exist in the codebase
- Validate API endpoint references against actual route definitions
- Check that CLI command examples match current `--help` output

**Link validation**:
- Internal link checking (files exist, anchors valid)
- External link checking (HTTP status codes)
- Image/asset reference validation

**Tools for drift detection**:
- **Swimm**: Commercial tool that links docs to code and alerts when referenced code changes (proprietary)
- **lychee / markdown-link-check**: Automated broken link detection
- **Custom git hooks**: Compare doc modification dates against code modification dates

### 3.3 Recommended Approach for AIDA Skill

Since the skill runs as a Claude Code analysis (not a persistent service), focus on:
1. **Static analysis**: Check for temporal language, TODO/TBD markers, version references
2. **Cross-reference**: Extract mentioned file paths, function names, CLI commands and verify they exist
3. **Git metadata**: Use `git log` to determine last modification dates and flag stale files
4. **Link validation**: Check internal links and anchors exist

---

## 4. Hype / Marketing Language Detection

### 4.1 Hype Word Categories

Based on analysis of Google style guide, Joblint, write-good, proselint, and Microsoft style guide, here is a comprehensive taxonomy:

#### Category 1: Superlatives and Intensifiers
```
amazing, awesome, beautiful, best, best-in-class, blazing, blazing-fast,
breakthrough, brilliant, cutting-edge, elegant, excellent, exceptional,
exciting, extraordinary, fantastic, groundbreaking, incredible, innovative,
lightning-fast, magical, mind-blowing, next-generation, outstanding,
perfect, phenomenal, powerful, premier, remarkable, revolutionary,
state-of-the-art, stunning, superior, tremendous, ultimate, unmatched,
unparalleled, unprecedented, world-class
```

#### Category 2: Vague Qualifiers (Weasel Words)
```
clearly, completely, easily, effortlessly, exceedingly, extremely, fairly,
highly, hugely, incredibly, interestingly, largely, mostly, naturally,
obviously, quite, really, relatively, remarkably, significantly, simply,
substantially, surprisingly, totally, truly, usually, vastly, very
```

#### Category 3: Corporate Buzzwords
```
actionable, agile (non-methodology), best practices, blue sky, boil the ocean,
circle back, deep dive, disruptive, ecosystem, empower, enable, end-to-end,
enterprise-grade, evangelize, game-changer, go-to-market, holistic, ideate,
incentivize, leverage, low-hanging fruit, mission-critical, move the needle,
next-level, paradigm, paradigm shift, pivot, productize, proactive, reach out,
robust, scalable, seamless, synergy, synergize, thought leader, touch base,
turnkey, value-add, visionary, world-class
```

#### Category 4: False Simplicity
```
easy, easily, effortless, just, obvious, obviously, of course, quick, quickly,
simple, simply, straightforward, trivial
```
*Why this matters*: What's easy for one person may be difficult for another. These words dismiss the reader's potential struggles.

#### Category 5: Temporal Hype
```
brand new, cutting-edge, latest, latest and greatest, modern, new, newest,
next-generation, state-of-the-art
```
*Why this matters*: Technical documentation should be timeless. These words date rapidly.

#### Category 6: Unsubstantiated Claims
```
battle-tested, best, enterprise-ready, fast, guaranteed, high-performance,
instant, lightning-fast, mature, optimized, performant, production-ready,
proven, reliable, rock-solid, scalable, secure, stable, zero-downtime
```
*Why this matters*: Without benchmarks or evidence, these are marketing claims, not documentation.

### 4.2 Detection Patterns

Beyond individual words, detect these *patterns*:
- **Unquantified performance claims**: "fast" without benchmarks, "scalable" without limits
- **Comparative without referent**: "better", "faster", "more powerful" -- compared to what?
- **Superlative stacking**: "the most powerful and innovative solution"
- **Feature-as-benefit confusion**: describing what something IS rather than what the user CAN DO
- **Exclamation marks**: almost never appropriate in technical documentation
- **Marketing qualifiers before nouns**: "powerful API", "robust framework", "elegant solution"

---

## 5. Quality Dimensions to Check

### 5.1 Structural Quality
- [ ] Document has a clear title (top-level heading)
- [ ] Heading hierarchy is correct (no skipped levels)
- [ ] No duplicate headings at the same level
- [ ] All sections have content (no empty sections)
- [ ] Code blocks have language specifiers
- [ ] Lists are properly formatted and consistent
- [ ] Tables have correct column counts
- [ ] Images have alt text
- [ ] File ends with a newline

### 5.2 Prose Quality
- [ ] Active voice preferred over passive
- [ ] No weasel words or hedging language
- [ ] No cliches
- [ ] No wordy phrases (replaceable with shorter alternatives)
- [ ] No lexical illusions (repeated words)
- [ ] Readability score within target (Flesch-Kincaid grade < 8, Gunning Fog < 10)
- [ ] Consistent terminology throughout
- [ ] No undefined acronyms (first use should expand)
- [ ] No unnecessary jargon

### 5.3 Tone and Voice
- [ ] No hype or marketing language
- [ ] No false simplicity ("just", "simply", "easy")
- [ ] No exclamation marks
- [ ] Consistent person (prefer "you" over "we")
- [ ] No condescending language
- [ ] No ableist, gendered, or exclusionary language
- [ ] Professional but approachable

### 5.4 Accuracy and Freshness
- [ ] No broken internal links
- [ ] No broken external links
- [ ] No references to non-existent files/functions/APIs
- [ ] No outdated version references
- [ ] No TODO/TBD/FIXME/HACK markers
- [ ] No "coming soon" or placeholder content
- [ ] No temporal language that will age ("currently", "recently", "new")
- [ ] Code examples are syntactically valid
- [ ] CLI command examples match current interface
- [ ] Screenshots match current UI (if applicable)

### 5.5 Completeness
- [ ] All documented features have examples
- [ ] Error cases and edge cases are covered
- [ ] Prerequisites are stated
- [ ] Related documents are cross-linked
- [ ] API parameters/return values are documented
- [ ] Configuration options are documented with defaults

### 5.6 Consistency
- [ ] Consistent capitalization of product/feature names
- [ ] Consistent code formatting (backticks for inline, fenced for blocks)
- [ ] Consistent list marker style
- [ ] Consistent heading style
- [ ] Consistent date formats
- [ ] Consistent terminology (don't mix synonyms for the same concept)

---

## 6. Specific Anti-Patterns to Detect

### 6.1 Structural Anti-Patterns
1. **Wall of text**: Paragraphs > 5 sentences without headings, lists, or code blocks
2. **Heading-only sections**: Heading followed immediately by another heading (no content)
3. **Deep nesting**: Lists nested > 3 levels deep
4. **Orphan headings**: Single subsection under a parent (if you have H3, you should have at least two)
5. **Missing introduction**: Document jumps straight into details without context
6. **Buried lede**: Key information appears deep in the document rather than early

### 6.2 Prose Anti-Patterns
1. **Passive voice overuse**: > 20% of sentences in passive voice
2. **Sentence length**: Sentences > 30 words
3. **Paragraph length**: Paragraphs > 150 words
4. **Nominalization**: Using noun forms of verbs ("perform an installation" vs "install")
5. **Double negatives**: "not uncommon", "not unlikely"
6. **Redundant phrases**: "end result", "future plans", "past history", "basic fundamentals"
7. **Latin terms when English exists**: "e.g." could be "for example", "i.e." could be "that is"
8. **Weasel hedging**: "some users might find", "it could potentially", "it is generally believed"

### 6.3 Technical Documentation Anti-Patterns
1. **Undocumented prerequisites**: Assumes reader has specific tools/knowledge without stating it
2. **Missing error handling**: Shows the happy path but not what to do when things fail
3. **Copy-paste unfriendly code**: Code blocks with `$` prompts, `>>>` prefixes, or line numbers
4. **Outdated screenshots**: UI screenshots that don't match current version
5. **Magic values**: Configuration examples with unexplained values
6. **Version pinning without rationale**: "Use version 2.3.1" without explaining why
7. **Platform assumptions**: Assumes specific OS, shell, or environment without stating it
8. **Incomplete examples**: Code snippets that can't run without missing context

### 6.4 Hype Anti-Patterns
1. **Adjective stuffing**: "Our powerful, robust, enterprise-grade, scalable solution"
2. **Unsubstantiated claims**: "Blazing fast" without benchmarks
3. **Competitive positioning in docs**: "Unlike other tools..." or "The best solution for..."
4. **Feature marketing**: Describing features in marketing terms rather than user terms
5. **Exclamation enthusiasm**: "Check out our amazing new feature!"
6. **Buzzword bingo**: Stacking multiple buzzwords in one sentence

---

## 7. Before/After Diff Presentation

### 7.1 Tools for Prose Diffing

| Tool | Type | Granularity | Notes |
|------|------|------------|-------|
| **jsdiff** (npm) | JavaScript library | char, word, word+space, line, sentence, CSS, JSON, array | Best for word-level prose diffs; `diffWords` and `diffSentences` ideal for docs |
| **diff-match-patch** | Multi-language lib | char, word, line | Google-developed (for Google Docs); available in 8 languages |
| **diff2html** | JavaScript library | line-by-line, side-by-side | Renders git/unified diffs as styled HTML |
| **git diff --word-diff** | Git built-in | word | `--word-diff=color` or `--word-diff=plain` for terminal output |

### 7.2 Recommended Diff Presentation for AIDA Skill

Since the AIDA skill runs in a terminal/Claude Code context, use markdown-formatted diffs:

**Format 1: Inline strikethrough/bold (for small changes)**
```
Before: The system ~~utilizes~~ **uses** a ~~robust~~ **reliable** architecture.
```

**Format 2: Before/After blocks (for larger changes)**
```markdown
**Before:**
> The system utilizes a robust and powerful architecture that enables
> seamless integration with enterprise-grade solutions.

**After:**
> The system uses a reliable architecture that integrates with
> existing solutions.

**Why:** Removed marketing language ("robust", "powerful", "seamless",
"enterprise-grade"), replaced "utilizes" with "uses", made claim specific.
```

**Format 3: Diff code block (for structural changes)**
```diff
- ## Getting Started With Our Amazing Platform
+ ## Getting Started

- Simply install the package and you're good to go!
+ Install the package:

  ```bash
- $ npm install awesome-package
+ npm install awesome-package
  ```
```

### 7.3 Presentation Strategy

For each issue found, present:
1. **Issue category** (from the taxonomy above)
2. **Severity** (error, warning, suggestion)
3. **Location** (file, line, section heading)
4. **Current text** (the problematic text)
5. **Suggested fix** (the improved text)
6. **Rationale** (why this is an issue, citing the relevant style guide principle)

---

## 8. Recommended Approach for AIDA `/aida-doc-review` Skill

### 8.1 Architecture

The skill should be a Claude Code skill (markdown template in `.claude/skills/`) that:
1. Accepts a file path, glob pattern, or directory as input
2. Reads the target documentation files
3. Performs multi-dimensional analysis
4. Outputs a structured report with findings and suggested fixes

### 8.2 Analysis Phases

**Phase 1: Structural Analysis** (fast, deterministic)
- Heading hierarchy validation
- Empty sections detection
- Code block language specifiers
- Link extraction and validation (internal)
- Image alt text presence
- List and table formatting
- Document length and section balance

**Phase 2: Prose Quality** (AI-powered)
- Passive voice detection
- Weasel word scanning (use the curated lists from Section 4)
- Readability assessment (sentence/paragraph length, complexity)
- Cliche detection
- Wordy phrase identification with replacements
- Lexical illusion detection (repeated words)

**Phase 3: Tone and Hype Detection** (AI-powered, use word lists)
- Scan against hype word categories (Section 4.1)
- Detect unsubstantiated claims
- Flag false simplicity language
- Check for exclamation marks
- Detect marketing patterns

**Phase 4: Freshness and Accuracy** (requires codebase context)
- Extract referenced file paths and verify they exist
- Extract referenced CLI commands and verify against `--help`
- Check for TODO/TBD/FIXME markers
- Check for temporal language
- Compare git modification dates of docs vs related code
- Validate internal document cross-references

**Phase 5: Consistency** (cross-document)
- Terminology consistency across documents
- Capitalization consistency for product/feature names
- Style consistency (list markers, heading styles, code formatting)
- Abbreviation/acronym consistency

### 8.3 Output Format

```markdown
# Documentation Review: [file(s)]

## Summary
- **Files reviewed**: N
- **Total issues**: N (E errors, W warnings, S suggestions)
- **Overall grade**: A-F (composite score)
- **Readability**: Grade level X (target: 8)

## Critical Issues (Errors)
### [Category]: [Description]
**File**: path/to/file.md, Line N
**Current**: `problematic text here`
**Suggested**: `improved text here`
**Rationale**: [why this matters]

## Warnings
...

## Suggestions
...

## Freshness Report
| File | Last Modified | Related Code Changed | Status |
|------|--------------|---------------------|--------|
| ... | ... | ... | Fresh/Stale/Unknown |

## Before/After Summary
[Aggregated diff showing all suggested changes]
```

### 8.4 Scoring System

Compute a composite score across dimensions:

| Dimension | Weight | What it measures |
|-----------|--------|-----------------|
| Structure | 20% | Heading hierarchy, formatting, completeness |
| Prose | 20% | Clarity, concision, readability |
| Tone | 15% | Professional, non-hype, inclusive |
| Accuracy | 25% | Links valid, references current, no stale content |
| Completeness | 10% | Examples present, error cases covered |
| Consistency | 10% | Terminology, style, formatting uniform |

Grade thresholds: A (90+), B (80-89), C (70-79), D (60-69), F (<60)

### 8.5 Configuration

Allow project-level configuration via `.aida/doc-review.yml`:
```yaml
# Target readability grade level
readability_target: 8

# Custom hype words to flag (added to defaults)
custom_hype_words:
  - "synergistic"
  - "paradigm-shifting"

# Words to allow (removed from defaults)
allowed_words:
  - "scalable"  # OK for our infrastructure docs

# Directories to scan
doc_paths:
  - "docs/"
  - "*.md"
  - "!node_modules/"

# Severity overrides
rules:
  hype_language: warning    # default: warning
  passive_voice: suggestion # default: warning
  broken_links: error       # default: error
```

### 8.6 Integration Points

- **Pre-commit hook**: Run lightweight checks (structure, hype words) before commit
- **CI pipeline**: Run full review on PR documentation changes
- **On-demand**: `aida doc-review path/to/docs/` for manual review
- **Claude Code skill**: `/aida-doc-review` for interactive review with AI-powered suggestions

---

## 9. Key Takeaways

1. **Vale's taxonomy is the right mental model**: existence, substitution, consistency, and metric checks cover the space well. We should organize our checks similarly.

2. **Word lists are the foundation**: The curated lists from write-good (weasel words), Google (word list), Joblint (hype), and proselint (corporate speak) provide hundreds of specific terms to flag. These should be embedded in the skill.

3. **Structure matters as much as prose**: markdownlint's 53+ rules demonstrate that formatting consistency is a separate quality dimension from writing quality.

4. **Freshness is the hardest problem**: No open-source tool fully solves documentation drift detection. The best approach combines git metadata analysis, cross-reference validation, and content-based heuristics (temporal language, TODO markers).

5. **Before/after diffs are essential**: Every finding should include a concrete fix. jsdiff's word-level diffing is the right granularity for prose changes. In a terminal context, markdown diff blocks and strikethrough formatting work well.

6. **Readability metrics are well-established**: Flesch-Kincaid grade level 8 and Gunning Fog 10 are the standard thresholds. These can be computed from word/sentence/syllable counts.

7. **The four documentation types** (Divio system) provide a useful lens for completeness review: does the project have tutorials, how-to guides, reference, and explanation?

8. **Inclusive language checking** (via alex's approach) is a non-negotiable quality dimension for modern documentation.

## Related Requirements
- To be created when skill implementation begins

## References

- [Vale](https://vale.sh) - Prose linter
- [markdownlint](https://github.com/DavidAnson/markdownlint) - Markdown structure linter
- [write-good](https://github.com/btford/write-good) - English prose linter
- [proselint](https://github.com/amperser/proselint) - Prose quality checker
- [alex](https://alexjs.com) - Inclusive language checker
- [textlint](https://github.com/textlint/textlint) - Pluggable natural language linter
- [remark-lint](https://github.com/remarkjs/remark-lint) - Markdown linter (unified ecosystem)
- [lychee](https://github.com/lycheeverse/lychee) - Fast link checker (Rust)
- [markdown-link-check](https://github.com/tcort/markdown-link-check) - Link validator
- [jsdiff](https://github.com/kpdecker/jsdiff) - JavaScript text diff library
- [diff-match-patch](https://github.com/google/diff-match-patch) - Google's diff library
- [diff2html](https://github.com/rtfpessoa/diff2html) - HTML diff renderer
- [Google Developer Documentation Style Guide](https://developers.google.com/style)
- [Microsoft Writing Style Guide](https://learn.microsoft.com/en-us/style-guide/)
- [Google Technical Writing Courses](https://developers.google.com/tech-writing)
- [Divio Documentation System](https://docs.divio.com/documentation-system/)
- [Write the Docs](https://www.writethedocs.org/guide/)
