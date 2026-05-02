# AIDA AI Integration Design Document

## Vision: Self-Improving Requirements Management

AIDA (AI Design Assistant) aims to become a **self-referential system** where AI assists in managing requirements, and those requirements can feed back to improve AIDA itself. This creates a virtuous cycle: better requirements lead to better tooling, which leads to even better requirements.

## Design Principles

### 1. Speed and Flow
- **Keyboard-first**: All AI actions accessible via keyboard shortcuts
- **Non-blocking**: AI operations should not freeze the UI
- **Progressive disclosure**: Quick actions immediately available, deeper analysis on demand
- **Inline results**: AI suggestions appear in context, not in separate windows

### 2. Editor-like Experience
- Think vim/emacs: modal, efficient, muscle-memory-friendly
- AI as a "pair programmer" for requirements, not a wizard/chatbot
- Results are suggestions, user has final control
- Undo/history for all AI-initiated changes

### 3. Context is Everything
- AI has full database context (requirements, relationships, history)
- AI understands the project's conventions from existing data
- AI learns from user acceptance/rejection of suggestions

## Architecture

### Integration Approaches (Ranked by Recommendation)

#### Option A: Claude Code CLI Integration (Recommended for MVP)
```
┌─────────────────────────────────────────────────────────────┐
│                        AIDA GUI                              │
│  ┌─────────────────────────────────────────────────────┐    │
│  │  Actions > AI > Evaluate Requirement                 │    │
│  │              > Suggest Relationships                 │    │
│  │              > Find Duplicates                       │    │
│  │              > Improve Description                   │    │
│  │              > Generate Sub-requirements             │    │
│  └─────────────────────────────────────────────────────┘    │
│                           │                                  │
│                           ▼                                  │
│  ┌─────────────────────────────────────────────────────┐    │
│  │              AI Request Builder                      │    │
│  │  - Serializes current requirement + context          │    │
│  │  - Builds prompt with instruction + data             │    │
│  │  - Spawns claude CLI process                         │    │
│  └─────────────────────────────────────────────────────┘    │
│                           │                                  │
└───────────────────────────┼──────────────────────────────────┘
                            │
                            ▼
            ┌───────────────────────────────┐
            │     claude (CLI)              │
            │  - Receives JSON context      │
            │  - Returns structured output  │
            └───────────────────────────────┘
```

**Pros:**
- Uses existing Claude Code installation (no API key management)
- Inherits user's Claude Code settings and context
- Simple subprocess spawning from Rust
- Can leverage claude's `--print` mode for non-interactive use

**Cons:**
- Requires Claude Code to be installed
- Subprocess overhead (acceptable for non-real-time operations)

#### Option B: Direct Anthropic API
```rust
// Direct API calls using reqwest
async fn call_claude_api(prompt: &str, context: &str) -> Result<String> {
    let client = reqwest::Client::new();
    let response = client.post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .json(&request_body)
        .send()
        .await?;
    // ...
}
```

**Pros:**
- Full control over API parameters
- No external dependencies beyond API key
- Can use streaming for real-time feedback

**Cons:**
- Requires API key management in settings
- Additional cost considerations
- More complex error handling

#### Option C: MCP (Model Context Protocol) Server
```
AIDA could expose an MCP server that Claude Code connects to, allowing
Claude Code to directly read/write requirements.
```

**Pros:**
- Two-way integration
- Claude Code can proactively assist
- Natural conversation flow

**Cons:**
- More complex architecture
- Requires understanding MCP server implementation
- Better for Phase 2+

### Recommended Approach: Phased Implementation

**Phase 1 (MVP):** Claude CLI integration with `--print` mode
**Phase 2:** Direct API for speed-critical operations
**Phase 3:** MCP server for full bidirectional integration

## Feature Design

### Phase 1: Core AI Actions

#### 1. Evaluate Requirement (Keyboard: `Ctrl+Shift+E` or `'a' 'e'`)
**Purpose:** Assess quality of current requirement

**Context Provided:**
- Current requirement (full JSON)
- All related requirements (parents, children, references)
- Project type definitions and conventions
- Sample of similar well-written requirements (for style reference)

**AI Response Format:**
```json
{
  "quality_score": 7,
  "issues": [
    {
      "type": "vague_language",
      "severity": "medium",
      "text": "Description uses 'should work properly' - not measurable",
      "suggestion": "Replace with specific acceptance criteria"
    }
  ],
  "strengths": ["Clear title", "Appropriate type classification"],
  "suggested_improvements": {
    "description": "...improved text...",
    "rationale": "Added measurable criteria"
  }
}
```

**UI Treatment:**
- Show in side panel or modal
- Highlight issues inline in requirement text
- One-click apply for suggested improvements

#### 2. Find Duplicates (Keyboard: `'a' 'd'`)
**Purpose:** Identify potential duplicate or overlapping requirements

**Context Provided:**
- Current requirement
- All other requirements (titles + descriptions)

**AI Response Format:**
```json
{
  "potential_duplicates": [
    {
      "spec_id": "FR-0045",
      "similarity": 0.85,
      "reason": "Both describe user authentication flow",
      "recommendation": "merge" | "link" | "keep_separate"
    }
  ]
}
```

**UI Treatment:**
- Show list with similarity scores
- Quick actions: Create Duplicate relationship, Jump to requirement, Dismiss

#### 3. Suggest Relationships (Keyboard: `'a' 'r'`)
**Purpose:** Propose missing relationships

**Context Provided:**
- Current requirement
- All requirements with their existing relationships
- Relationship type definitions

**AI Response Format:**
```json
{
  "suggested_relationships": [
    {
      "rel_type": "depends_on",
      "target_spec_id": "FR-0023",
      "confidence": 0.9,
      "rationale": "This feature requires authentication to be implemented first"
    }
  ]
}
```

**UI Treatment:**
- Show in Links tab with "AI Suggested" badge
- One-click to create relationship
- Dismiss to hide suggestion

#### 4. Improve Description (Keyboard: `'a' 'i'`)
**Purpose:** Enhance requirement clarity and completeness

**Context Provided:**
- Current requirement
- Type-specific templates/conventions
- Project domain context

**AI Response Format:**
```json
{
  "improved_description": "...",
  "changes_made": [
    "Added acceptance criteria",
    "Clarified scope boundaries",
    "Added examples"
  ],
  "diff": "..." // Optional: show what changed
}
```

**UI Treatment:**
- Show diff view (original vs improved)
- Accept/Reject buttons
- Option to accept partial changes

#### 5. Generate Sub-requirements (Keyboard: `'a' 'g'`)
**Purpose:** Break down high-level requirement into implementable pieces

**Context Provided:**
- Current requirement
- Existing children (if any)
- Project's typical requirement granularity

**AI Response Format:**
```json
{
  "suggested_children": [
    {
      "title": "Implement login form UI",
      "description": "...",
      "type": "Functional",
      "rationale": "Separates UI from backend logic"
    }
  ]
}
```

**UI Treatment:**
- Preview list of suggested requirements
- Checkbox to select which to create
- Bulk create as children

### Phase 2: Batch Operations

#### 6. Analyze All Requirements
- Quality audit across entire database
- Generate report of issues by severity
- Prioritized list of requirements needing attention

#### 7. Consistency Check
- Verify naming conventions
- Check relationship integrity
- Identify orphaned requirements
- Status workflow violations

#### 8. Generate Traceability Matrix
- AI-assisted matrix generation
- Highlight gaps in coverage
- Suggest missing verification requirements

### Phase 3: Self-Referential Capabilities

#### 9. Generate AIDA Improvement Requirements
**The Meta Feature:** When working on AIDA's own requirements database:
- AI can suggest new features based on usage patterns
- AI can identify missing requirements for existing features
- AI can propose UX improvements based on workflow analysis

#### 10. Export to Claude Code
- Generate Claude Code prompt for implementing a requirement
- Include relevant context from related requirements
- Format for direct paste into Claude Code session

## Implementation Details

### AI Request Module (`aida-core/src/ai/`)

```rust
// ai/mod.rs
pub mod client;
pub mod prompts;
pub mod responses;

// ai/client.rs
pub struct AiClient {
    mode: AiMode,
}

pub enum AiMode {
    ClaudeCli { path: PathBuf },
    DirectApi { api_key: String },
}

impl AiClient {
    pub async fn evaluate_requirement(
        &self,
        req: &Requirement,
        context: &RequirementsStore,
    ) -> Result<EvaluationResponse> {
        let prompt = prompts::build_evaluation_prompt(req, context);
        let response = self.send_request(&prompt).await?;
        responses::parse_evaluation(response)
    }

    async fn send_request(&self, prompt: &str) -> Result<String> {
        match &self.mode {
            AiMode::ClaudeCli { path } => {
                // Spawn claude process with --print flag
                let output = Command::new(path)
                    .arg("--print")
                    .arg("-p")
                    .arg(prompt)
                    .output()
                    .await?;
                String::from_utf8(output.stdout)
            }
            AiMode::DirectApi { api_key } => {
                // HTTP request to Anthropic API
                todo!()
            }
        }
    }
}

// ai/prompts.rs
pub fn build_evaluation_prompt(req: &Requirement, store: &RequirementsStore) -> String {
    format!(r#"
You are evaluating a software requirement for quality and completeness.

## Requirement to Evaluate
{req_json}

## Related Requirements
{related_json}

## Project Context
- Total requirements: {total}
- Type definitions: {types}

## Instructions
Evaluate this requirement and provide:
1. Quality score (1-10)
2. Issues found with severity and suggestions
3. Strengths
4. Improved description if needed

Respond in JSON format:
```json
{{
  "quality_score": ...,
  "issues": [...],
  "strengths": [...],
  "suggested_improvements": {{...}}
}}
```
"#,
        req_json = serde_json::to_string_pretty(req)?,
        related_json = get_related_requirements_json(req, store),
        total = store.requirements.len(),
        types = get_type_definitions_summary(store),
    )
}
```

### GUI Integration (`aida-gui/src/ai_panel.rs`)

```rust
// State for AI operations
pub struct AiState {
    pub pending_request: Option<AiRequestType>,
    pub last_response: Option<AiResponse>,
    pub show_results_panel: bool,
    pub is_loading: bool,
}

pub enum AiRequestType {
    Evaluate(Uuid),
    FindDuplicates(Uuid),
    SuggestRelationships(Uuid),
    ImproveDescription(Uuid),
    GenerateChildren(Uuid),
}

// In app.rs - add to Actions menu
ui.menu_button("🤖 AI", |ui| {
    ui.set_min_width(200.0);

    if ui.button("📊 Evaluate Requirement  (ae)").clicked() {
        self.trigger_ai_action(AiRequestType::Evaluate(req_id));
        ui.close_menu();
    }

    if ui.button("🔍 Find Duplicates       (ad)").clicked() {
        self.trigger_ai_action(AiRequestType::FindDuplicates(req_id));
        ui.close_menu();
    }

    if ui.button("🔗 Suggest Relationships (ar)").clicked() {
        self.trigger_ai_action(AiRequestType::SuggestRelationships(req_id));
        ui.close_menu();
    }

    ui.separator();

    if ui.button("✨ Improve Description   (ai)").clicked() {
        self.trigger_ai_action(AiRequestType::ImproveDescription(req_id));
        ui.close_menu();
    }

    if ui.button("📝 Generate Children     (ag)").clicked() {
        self.trigger_ai_action(AiRequestType::GenerateChildren(req_id));
        ui.close_menu();
    }
});
```

### Keyboard Shortcuts

Following the existing pattern of 's' for status, 'p' for priority:

| Shortcut | Action |
|----------|--------|
| `a e` | AI: Evaluate requirement |
| `a d` | AI: Find duplicates |
| `a r` | AI: Suggest relationships |
| `a i` | AI: Improve description |
| `a g` | AI: Generate children |
| `a a` | AI: Analyze all (batch) |

The `a` prefix creates an "AI mode" similar to vim command modes.

### Settings Integration

Add to Settings > AI tab:
- **AI Provider**: Claude CLI / Direct API / Disabled
- **Claude CLI Path**: Auto-detect or manual
- **API Key**: (for Direct API mode)
- **Model**: claude-3-opus / claude-3-sonnet / etc.
- **Context Size**: How much database to include
- **Auto-suggest**: Enable/disable passive AI suggestions

### Results Panel

A new panel (or tab in Details view) for AI results:

```
┌─────────────────────────────────────────┐
│ 🤖 AI Analysis                      [×] │
├─────────────────────────────────────────┤
│ Quality Score: 7/10                     │
│                                         │
│ ⚠️ Issues (2)                           │
│ ├─ Medium: Vague language in desc       │
│ │  → "should work properly" not         │
│ │    measurable                         │
│ │  [Apply Fix]                          │
│ └─ Low: Missing acceptance criteria     │
│    [Add Template]                       │
│                                         │
│ ✅ Strengths                             │
│ ├─ Clear, concise title                 │
│ └─ Correct type classification          │
│                                         │
│ [Apply All Suggestions] [Dismiss]       │
└─────────────────────────────────────────┘
```

## Data Flow

```
User Action (menu or keyboard)
        │
        ▼
Build Context (serialize relevant data)
        │
        ▼
Build Prompt (instruction + context)
        │
        ▼
Send to AI (async, non-blocking)
        │
        ▼
Parse Response (validate JSON structure)
        │
        ▼
Update UI (show results panel)
        │
        ▼
User Decision (accept/reject/modify)
        │
        ▼
Apply Changes (if accepted, with history tracking)
```

## Context Optimization

For large databases, we need smart context selection:

1. **Always include:**
   - Current requirement (full)
   - Direct relationships (parents, children)
   - Project metadata (types, conventions)

2. **Include if relevant:**
   - Requirements in same feature
   - Requirements with similar tags
   - Recent requirements (for style reference)

3. **Summarize:**
   - Full list of spec_ids and titles (for duplicate detection)
   - Statistics (counts by type, status, feature)

4. **Exclude:**
   - Archived requirements (unless specifically relevant)
   - Full history (unless analyzing history)

## Error Handling

```rust
pub enum AiError {
    CliNotFound,
    CliExecFailed(String),
    ApiKeyMissing,
    ApiRequestFailed(reqwest::Error),
    InvalidResponse(String),
    RateLimited,
    ContextTooLarge,
}

// Graceful degradation
impl AiClient {
    pub fn is_available(&self) -> bool {
        match &self.mode {
            AiMode::ClaudeCli { path } => path.exists(),
            AiMode::DirectApi { api_key } => !api_key.is_empty(),
        }
    }
}
```

## Testing Strategy

1. **Unit tests:** Prompt building, response parsing
2. **Integration tests:** Mock AI responses, verify state changes
3. **Manual testing:** Real AI interactions (expensive, do sparingly)

## Future Considerations

### Streaming Responses
For long operations, stream tokens to show progress:
```rust
// Show partial results as they arrive
async fn stream_ai_response(&self, prompt: &str, on_token: impl Fn(&str)) {
    // ...
}
```

### Learning from Feedback
Track user acceptance/rejection to improve prompts:
```rust
struct AiFeedback {
    request_type: AiRequestType,
    was_accepted: bool,
    was_modified: bool,
    modification_extent: f32, // 0.0 = accepted as-is, 1.0 = completely rewritten
}
```

### Embedding-based Duplicate Detection
For very large databases, use embeddings for similarity search instead of sending all requirements to AI.

## Implementation Order

1. **Week 1:**
   - Add AI submenu to Actions dropdown
   - Implement Claude CLI detection and basic invocation
   - Build `evaluate_requirement` prompt and response parsing

2. **Week 2:**
   - Add AI results panel
   - Implement keyboard shortcuts (`a` prefix mode)
   - Add Settings > AI tab

3. **Week 3:**
   - Implement remaining actions (duplicates, relationships, improve, generate)
   - Add "Apply" functionality with history tracking

4. **Week 4:**
   - Polish UX (loading states, error messages)
   - Batch operations
   - Documentation

## Conclusion

This design prioritizes:
- **Speed:** Keyboard shortcuts, non-blocking operations
- **Control:** User always approves changes
- **Context:** AI has rich understanding of the project
- **Iteration:** Start simple (CLI), evolve to more sophisticated integration

The self-referential nature of AIDA managing its own requirements creates a unique opportunity: every improvement to AIDA makes it better at suggesting further improvements.
