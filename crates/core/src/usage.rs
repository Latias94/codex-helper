use serde::{Deserialize, Serialize};
use serde_json::Value;

fn i64_is_zero(value: &i64) -> bool {
    *value == 0
}

fn economics_status_is_complete(value: &EconomicsStatus) -> bool {
    *value == EconomicsStatus::Complete
}

fn usage_total_source_is_derived(value: &UsageTotalSource) -> bool {
    *value == UsageTotalSource::Derived
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UsageEvidenceSource {
    ResponsesInputTokensDetailsCachedTokens,
    ChatPromptTokensDetailsCachedTokens,
    CachedInputTokensAlias,
    CachedTokensAlias,
    CacheReadInputTokensAlias,
    CacheReadTokensAlias,
    ResponsesInputTokensDetailsCacheWriteTokens,
    ChatPromptTokensDetailsCacheWriteTokens,
    ResponsesInputTokensDetailsCacheCreationTokens,
    ChatPromptTokensDetailsCacheCreationTokens,
    CacheCreationInputTokensAlias,
    CacheWriteInputTokensAlias,
    CacheCreationTokensAlias,
    CacheWriteTokensAlias,
    AnthropicCacheCreationTtl,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageTokenObservation {
    pub source: UsageEvidenceSource,
    pub value: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UsageEvidenceState {
    #[default]
    Missing,
    PresentZero,
    PresentValue,
    Invalid,
    Conflict,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
pub struct UsageTokenEvidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selected: Option<UsageTokenObservation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    observations: Vec<UsageTokenObservation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    invalid_sources: Vec<UsageEvidenceSource>,
}

#[doc(hidden)]
#[derive(Serialize)]
pub struct UsageTokenEvidenceWire<'a> {
    state: UsageEvidenceState,
    selected: &'a Option<UsageTokenObservation>,
    observations: &'a [UsageTokenObservation],
    invalid_sources: &'a [UsageEvidenceSource],
}

impl Serialize for UsageTokenEvidence {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        UsageTokenEvidenceWire {
            state: self.state(),
            selected: &self.selected,
            observations: &self.observations,
            invalid_sources: &self.invalid_sources,
        }
        .serialize(serializer)
    }
}

impl UsageTokenEvidence {
    pub fn selected(&self) -> Option<UsageTokenObservation> {
        self.selected
    }

    pub fn state(&self) -> UsageEvidenceState {
        if self.has_conflict() {
            UsageEvidenceState::Conflict
        } else if !self.invalid_sources.is_empty() {
            UsageEvidenceState::Invalid
        } else {
            match self.selected {
                None => UsageEvidenceState::Missing,
                Some(observation) if observation.value == 0 => UsageEvidenceState::PresentZero,
                Some(_) => UsageEvidenceState::PresentValue,
            }
        }
    }

    pub fn has_conflict(&self) -> bool {
        let Some(selected) = self.selected else {
            return false;
        };
        self.observations
            .iter()
            .any(|observation| observation.value != selected.value)
    }

    fn has_invalid_invariants(&self) -> bool {
        (self.selected.is_none() && !self.observations.is_empty())
            || self.selected.is_some_and(|selected| {
                selected.value < 0 || !self.observations.contains(&selected)
            })
            || self
                .observations
                .iter()
                .any(|observation| observation.value < 0)
    }

    pub fn is_present(&self) -> bool {
        self.selected.is_some() || !self.invalid_sources.is_empty()
    }

    fn observe(&mut self, source: UsageEvidenceSource, value: &Value) {
        let Some(value) = token_count(value) else {
            self.invalid_sources.push(source);
            return;
        };
        let observation = UsageTokenObservation { source, value };
        self.selected.get_or_insert(observation);
        self.observations.push(observation);
    }

    fn observe_value(&mut self, source: UsageEvidenceSource, value: i64) {
        let observation = UsageTokenObservation { source, value };
        self.selected.get_or_insert(observation);
        self.observations.push(observation);
    }

    fn mark_invalid(&mut self, source: UsageEvidenceSource) {
        self.invalid_sources.push(source);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct UsageEvidence {
    #[serde(default)]
    pub cache_read_input_tokens: UsageTokenEvidence,
    #[serde(default)]
    pub cache_write_input_tokens: UsageTokenEvidence,
    #[serde(default, skip_serializing_if = "economics_status_is_complete")]
    pub aggregate_status: EconomicsStatus,
}

impl UsageEvidence {
    fn is_empty(&self) -> bool {
        !self.cache_read_input_tokens.is_present()
            && !self.cache_write_input_tokens.is_present()
            && self.aggregate_status == EconomicsStatus::Complete
    }

    pub fn economics_status(&self) -> EconomicsStatus {
        if self.aggregate_status == EconomicsStatus::Conflict
            || self.cache_read_input_tokens.has_invalid_invariants()
            || self.cache_write_input_tokens.has_invalid_invariants()
            || matches!(
                self.cache_read_input_tokens.state(),
                UsageEvidenceState::Invalid | UsageEvidenceState::Conflict
            )
            || matches!(
                self.cache_write_input_tokens.state(),
                UsageEvidenceState::Invalid | UsageEvidenceState::Conflict
            )
        {
            EconomicsStatus::Conflict
        } else {
            self.aggregate_status
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CacheTokenInclusion {
    Unknown,
    Separate,
    IncludedInInput,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct CacheAccountingConvention {
    pub cache_read: CacheTokenInclusion,
    pub cache_write: CacheTokenInclusion,
}

impl CacheAccountingConvention {
    pub const UNKNOWN: Self = Self {
        cache_read: CacheTokenInclusion::Unknown,
        cache_write: CacheTokenInclusion::Unknown,
    };

    pub const SEPARATE: Self = Self {
        cache_read: CacheTokenInclusion::Separate,
        cache_write: CacheTokenInclusion::Separate,
    };

    pub const INCLUDED_IN_INPUT: Self = Self {
        cache_read: CacheTokenInclusion::IncludedInInput,
        cache_write: CacheTokenInclusion::IncludedInInput,
    };
}

impl Default for CacheAccountingConvention {
    fn default() -> Self {
        Self::UNKNOWN
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EconomicsStatus {
    #[default]
    Complete,
    Partial,
    Conflict,
}

impl EconomicsStatus {
    pub(crate) fn combine(self, other: Self) -> Self {
        match (self, other) {
            (Self::Conflict, _) | (_, Self::Conflict) => Self::Conflict,
            (Self::Partial, _) | (_, Self::Partial) => Self::Partial,
            _ => Self::Complete,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UsageTotalSource {
    #[default]
    Derived,
    DerivedWithoutConvention,
    Reported,
    Aggregated,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct CanonicalUsageBuckets {
    pub ordinary_input_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub cache_write_input_tokens: i64,
    pub status: EconomicsStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct UsageMetrics {
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default)]
    pub reasoning_tokens: i64,
    #[serde(default, skip_serializing_if = "i64_is_zero")]
    pub reasoning_output_tokens: i64,
    #[serde(default)]
    pub total_tokens: i64,
    #[serde(default, skip_serializing_if = "usage_total_source_is_derived")]
    pub total_tokens_source: UsageTotalSource,
    #[serde(default, skip_serializing_if = "i64_is_zero")]
    pub cached_input_tokens: i64,
    #[serde(
        default,
        alias = "cache_read_tokens",
        skip_serializing_if = "i64_is_zero"
    )]
    pub cache_read_input_tokens: i64,
    #[serde(
        default,
        alias = "cache_creation_tokens",
        skip_serializing_if = "i64_is_zero"
    )]
    pub cache_creation_input_tokens: i64,
    #[serde(default, skip_serializing_if = "i64_is_zero")]
    pub cache_creation_5m_input_tokens: i64,
    #[serde(default, skip_serializing_if = "i64_is_zero")]
    pub cache_creation_1h_input_tokens: i64,
    #[serde(default, skip_serializing_if = "UsageEvidence::is_empty")]
    pub evidence: UsageEvidence,
}

impl UsageMetrics {
    pub fn add_assign(&mut self, other: &UsageMetrics) {
        let mut aggregate_status = self.integrity_status().combine(other.integrity_status());
        let mut overflowed = false;
        self.input_tokens =
            aggregate_token_count(self.input_tokens, other.input_tokens, &mut overflowed);
        self.output_tokens =
            aggregate_token_count(self.output_tokens, other.output_tokens, &mut overflowed);
        self.reasoning_tokens = aggregate_token_count(
            self.reasoning_tokens,
            other.reasoning_tokens,
            &mut overflowed,
        );
        self.reasoning_output_tokens = aggregate_token_count(
            self.reasoning_output_tokens,
            other.reasoning_output_tokens_total(),
            &mut overflowed,
        );
        self.total_tokens =
            aggregate_token_count(self.total_tokens, other.total_tokens, &mut overflowed);
        self.cached_input_tokens = aggregate_token_count(
            self.cached_input_tokens,
            other.cached_input_tokens,
            &mut overflowed,
        );
        self.cache_read_input_tokens = aggregate_token_count(
            self.cache_read_input_tokens,
            other.cache_read_input_tokens,
            &mut overflowed,
        );
        self.cache_creation_input_tokens = aggregate_token_count(
            self.cache_creation_input_tokens,
            other.cache_creation_input_tokens,
            &mut overflowed,
        );
        self.cache_creation_5m_input_tokens = aggregate_token_count(
            self.cache_creation_5m_input_tokens,
            other.cache_creation_5m_input_tokens,
            &mut overflowed,
        );
        self.cache_creation_1h_input_tokens = aggregate_token_count(
            self.cache_creation_1h_input_tokens,
            other.cache_creation_1h_input_tokens,
            &mut overflowed,
        );
        if overflowed {
            aggregate_status = EconomicsStatus::Conflict;
        }
        self.total_tokens_source = UsageTotalSource::Aggregated;
        self.evidence = UsageEvidence {
            aggregate_status,
            ..UsageEvidence::default()
        };
    }

    pub fn reasoning_output_tokens_total(&self) -> i64 {
        self.reasoning_output_tokens.max(self.reasoning_tokens)
    }

    pub fn cache_creation_tokens_total(&self) -> i64 {
        if let Some(observation) = self.evidence.cache_write_input_tokens.selected() {
            return observation.value;
        }
        let by_ttl = self
            .cache_creation_5m_input_tokens
            .saturating_add(self.cache_creation_1h_input_tokens);
        self.cache_creation_input_tokens.max(by_ttl)
    }

    pub fn has_cache_tokens(&self) -> bool {
        self.cached_input_tokens > 0
            || self.cache_read_input_tokens > 0
            || self.cache_creation_tokens_total() > 0
    }

    pub fn cache_read_tokens_total(&self) -> i64 {
        if let Some(observation) = self.evidence.cache_read_input_tokens.selected() {
            return observation.value;
        }
        self.cached_input_tokens
            .max(0)
            .saturating_add(self.cache_read_input_tokens.max(0))
    }

    pub fn canonical_usage_buckets(
        &self,
        convention: CacheAccountingConvention,
    ) -> CanonicalUsageBuckets {
        let input = self.input_tokens.max(0);
        let read = self
            .evidence
            .cache_read_input_tokens
            .selected()
            .map(|observation| observation.value)
            .unwrap_or_else(|| self.cache_read_tokens_total())
            .max(0);
        let write = self
            .evidence
            .cache_write_input_tokens
            .selected()
            .map(|observation| observation.value)
            .unwrap_or_else(|| self.cache_creation_tokens_total())
            .max(0);

        let mut status = if self.integrity_status() == EconomicsStatus::Conflict
            || self.input_tokens < 0
            || self.cached_input_tokens < 0
            || self.cache_read_input_tokens < 0
            || self.cache_creation_input_tokens < 0
            || self.cache_creation_5m_input_tokens < 0
            || self.cache_creation_1h_input_tokens < 0
        {
            EconomicsStatus::Conflict
        } else {
            self.integrity_status()
        };

        let included_read = included_cache_tokens(convention.cache_read, read, &mut status);
        let included_write = included_cache_tokens(convention.cache_write, write, &mut status);
        let ordinary_input_tokens = match included_read
            .checked_add(included_write)
            .and_then(|included_cache| input.checked_sub(included_cache))
        {
            Some(value) if value >= 0 => value,
            _ => {
                status = EconomicsStatus::Conflict;
                0
            }
        };

        CanonicalUsageBuckets {
            ordinary_input_tokens,
            cache_read_input_tokens: read,
            cache_write_input_tokens: write,
            status,
        }
    }

    pub fn cache_hit_rate_with_convention(
        &self,
        convention: CacheAccountingConvention,
    ) -> Option<f64> {
        let buckets = self.canonical_usage_buckets(convention);
        let denominator = buckets
            .ordinary_input_tokens
            .checked_add(buckets.cache_read_input_tokens)
            .and_then(|value| value.checked_add(buckets.cache_write_input_tokens));
        if buckets.status != EconomicsStatus::Complete
            || !denominator.is_some_and(|value| value > 0)
        {
            return None;
        }
        Some(buckets.cache_read_input_tokens as f64 / denominator? as f64)
    }

    fn derived_total_components(&self) -> (i64, i64, UsageTotalSource) {
        let selected_cache_read = self.evidence.cache_read_input_tokens.selected();
        let selected_cache_write = self.evidence.cache_write_input_tokens.selected();
        let cache_write_is_included = selected_cache_write
            .is_some_and(|observation| source_uses_input_details_projection(observation.source));
        let cache_write_is_separate = selected_cache_write.is_some_and(|observation| {
            observation.source == UsageEvidenceSource::AnthropicCacheCreationTtl
        });
        let cache_write_is_ambiguous = self.cache_creation_tokens_total() != 0
            && !cache_write_is_included
            && !cache_write_is_separate;
        let direct_cache_read = selected_cache_read
            .filter(|observation| !source_uses_cached_projection(observation.source))
            .map(|observation| observation.value)
            .unwrap_or_else(|| self.cache_read_input_tokens.max(0));
        let cache_read_is_ambiguous =
            direct_cache_read != 0 && !cache_write_is_included && !cache_write_is_separate;
        let (separate_cache_read, separate_cache_write) = if cache_write_is_separate {
            (direct_cache_read, self.cache_creation_tokens_total())
        } else {
            (0, 0)
        };
        let source = if cache_write_is_ambiguous || cache_read_is_ambiguous {
            UsageTotalSource::DerivedWithoutConvention
        } else {
            UsageTotalSource::Derived
        };
        (separate_cache_read, separate_cache_write, source)
    }

    fn checked_derived_total(&self) -> (Option<i64>, UsageTotalSource) {
        let (separate_cache_read, separate_cache_write, source) = self.derived_total_components();
        let total = self
            .input_tokens
            .checked_add(self.output_tokens)
            .and_then(|value| value.checked_add(separate_cache_read))
            .and_then(|value| value.checked_add(separate_cache_write));
        (total, source)
    }

    fn derived_total(&self) -> (i64, UsageTotalSource) {
        let (separate_cache_read, separate_cache_write, source) = self.derived_total_components();
        let total = self
            .input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(separate_cache_read)
            .saturating_add(separate_cache_write);
        (total, source)
    }

    fn integrity_status(&self) -> EconomicsStatus {
        let ttl_overflow = self
            .cache_creation_5m_input_tokens
            .checked_add(self.cache_creation_1h_input_tokens)
            .is_none();
        let read_overflow = self
            .cached_input_tokens
            .max(0)
            .checked_add(self.cache_read_input_tokens.max(0))
            .is_none();
        let projection_conflict = self
            .evidence
            .cache_read_input_tokens
            .selected()
            .is_some_and(|selected| {
                if source_uses_cached_projection(selected.source) {
                    self.cached_input_tokens != selected.value
                } else {
                    self.cache_read_input_tokens != selected.value
                }
            })
            || self
                .evidence
                .cache_write_input_tokens
                .selected()
                .is_some_and(|selected| self.cache_creation_input_tokens != selected.value);
        let negative_token_count = self.input_tokens < 0
            || self.output_tokens < 0
            || self.reasoning_tokens < 0
            || self.reasoning_output_tokens < 0
            || self.total_tokens < 0
            || self.cached_input_tokens < 0
            || self.cache_read_input_tokens < 0
            || self.cache_creation_input_tokens < 0
            || self.cache_creation_5m_input_tokens < 0
            || self.cache_creation_1h_input_tokens < 0;
        let (expected_total, expected_source) = self.checked_derived_total();
        let total_overflow = expected_total.is_none();
        let reported_total_conflict = if self.total_tokens_source == UsageTotalSource::Reported {
            match (expected_total, expected_source) {
                (Some(expected), UsageTotalSource::Derived) => self.total_tokens != expected,
                (Some(minimum), UsageTotalSource::DerivedWithoutConvention) => {
                    self.total_tokens < minimum
                }
                (None, _) => true,
                _ => false,
            }
        } else {
            false
        };

        if ttl_overflow
            || read_overflow
            || projection_conflict
            || negative_token_count
            || total_overflow
            || reported_total_conflict
        {
            EconomicsStatus::Conflict
        } else {
            self.evidence.economics_status()
        }
    }
}

fn included_cache_tokens(
    inclusion: CacheTokenInclusion,
    tokens: i64,
    status: &mut EconomicsStatus,
) -> i64 {
    match inclusion {
        CacheTokenInclusion::IncludedInInput => tokens,
        CacheTokenInclusion::Separate => 0,
        CacheTokenInclusion::Unknown => {
            if tokens > 0 {
                *status = status.combine(EconomicsStatus::Partial);
            }
            0
        }
    }
}

fn aggregate_token_count(left: i64, right: i64, overflowed: &mut bool) -> i64 {
    match left.checked_add(right) {
        Some(value) => value,
        None => {
            *overflowed = true;
            left.saturating_add(right)
        }
    }
}

fn main_token_count(value: &Value, evidence: &mut UsageEvidence) -> i64 {
    match token_count(value) {
        Some(value) => value,
        None => {
            evidence.aggregate_status = EconomicsStatus::Conflict;
            0
        }
    }
}

fn observe_main_token_field(
    value: &Value,
    selected: &mut i64,
    present: &mut bool,
    evidence: &mut UsageEvidence,
) {
    let observed = main_token_count(value, evidence);
    if *present {
        if *selected != observed {
            evidence.aggregate_status = EconomicsStatus::Conflict;
        }
    } else {
        *selected = observed;
        *present = true;
    }
}

fn token_count(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64().filter(|value| *value >= 0),
        Value::String(value) => value.trim().parse::<i64>().ok().filter(|value| *value >= 0),
        _ => None,
    }
}

fn observe_token_field(
    evidence: &mut UsageTokenEvidence,
    object: Option<&serde_json::Map<String, Value>>,
    field: &str,
    source: UsageEvidenceSource,
) -> bool {
    let Some(value) = object.and_then(|object| object.get(field)) else {
        return false;
    };
    evidence.observe(source, value);
    true
}

fn source_uses_cached_projection(source: UsageEvidenceSource) -> bool {
    matches!(
        source,
        UsageEvidenceSource::ResponsesInputTokensDetailsCachedTokens
            | UsageEvidenceSource::ChatPromptTokensDetailsCachedTokens
            | UsageEvidenceSource::CachedInputTokensAlias
            | UsageEvidenceSource::CachedTokensAlias
    )
}

fn source_uses_input_details_projection(source: UsageEvidenceSource) -> bool {
    matches!(
        source,
        UsageEvidenceSource::ResponsesInputTokensDetailsCacheWriteTokens
            | UsageEvidenceSource::ChatPromptTokensDetailsCacheWriteTokens
            | UsageEvidenceSource::ResponsesInputTokensDetailsCacheCreationTokens
            | UsageEvidenceSource::ChatPromptTokensDetailsCacheCreationTokens
    )
}

#[derive(Debug, Default)]
struct UsageFieldPresence {
    input_tokens: bool,
    output_tokens: bool,
    reasoning_tokens: bool,
    reasoning_output_tokens: bool,
    total_tokens: bool,
}

#[derive(Debug)]
struct UsageUpdate {
    metrics: UsageMetrics,
    presence: UsageFieldPresence,
}

fn extract_usage_obj(payload: &Value) -> Option<&Value> {
    if let Some(u) = payload.get("usage") {
        return Some(u);
    }
    if let Some(msg) = payload.get("message")
        && let Some(u) = msg.get("usage")
    {
        return Some(u);
    }
    if let Some(resp) = payload.get("response")
        && let Some(u) = resp.get("usage")
    {
        return Some(u);
    }
    None
}

fn usage_from_value(usage_obj: &Value) -> Option<UsageMetrics> {
    usage_update_from_value(usage_obj).map(|update| update.metrics)
}

fn usage_update_from_value(usage_obj: &Value) -> Option<UsageUpdate> {
    let mut m = UsageMetrics::default();
    let mut presence = UsageFieldPresence::default();
    let mut recognized = false;

    // Canonical fields are visited first, so aliases can confirm but never overwrite them.
    for field in ["input_tokens", "prompt_tokens"] {
        if let Some(value) = usage_obj.get(field) {
            observe_main_token_field(
                value,
                &mut m.input_tokens,
                &mut presence.input_tokens,
                &mut m.evidence,
            );
            recognized = true;
        }
    }
    for field in ["output_tokens", "completion_tokens"] {
        if let Some(value) = usage_obj.get(field) {
            observe_main_token_field(
                value,
                &mut m.output_tokens,
                &mut presence.output_tokens,
                &mut m.evidence,
            );
            recognized = true;
        }
    }
    if let Some(v) = usage_obj.get("total_tokens") {
        m.total_tokens = main_token_count(v, &mut m.evidence);
        m.total_tokens_source = UsageTotalSource::Reported;
        presence.total_tokens = true;
        recognized = true;
    }

    // Some providers may expose reasoning tokens directly.
    if let Some(v) = usage_obj.get("reasoning_tokens") {
        let value = main_token_count(v, &mut m.evidence);
        m.reasoning_tokens = value;
        m.reasoning_output_tokens = value;
        presence.reasoning_tokens = true;
        presence.reasoning_output_tokens = true;
        recognized = true;
    }
    if let Some(v) = usage_obj.get("reasoning_output_tokens") {
        let value = main_token_count(v, &mut m.evidence);
        m.reasoning_output_tokens = value;
        m.reasoning_tokens = m.reasoning_tokens.max(value);
        presence.reasoning_tokens = true;
        presence.reasoning_output_tokens = true;
        recognized = true;
    }

    if let Some(details) = usage_obj
        .get("output_tokens_details")
        .and_then(|v| v.as_object())
        && let Some(v) = details.get("reasoning_tokens")
    {
        let value = main_token_count(v, &mut m.evidence);
        m.reasoning_tokens = value;
        m.reasoning_output_tokens = value;
        presence.reasoning_tokens = true;
        presence.reasoning_output_tokens = true;
        recognized = true;
    }
    if let Some(details) = usage_obj
        .get("completion_tokens_details")
        .and_then(|v| v.as_object())
        && let Some(v) = details.get("reasoning_tokens")
    {
        let value = main_token_count(v, &mut m.evidence);
        m.reasoning_tokens = value;
        m.reasoning_output_tokens = value;
        presence.reasoning_tokens = true;
        presence.reasoning_output_tokens = true;
        recognized = true;
    }

    let responses_input_details = usage_obj
        .get("input_tokens_details")
        .or_else(|| usage_obj.get("input_token_details"))
        .and_then(Value::as_object);
    let chat_prompt_details = usage_obj
        .get("prompt_tokens_details")
        .or_else(|| usage_obj.get("prompt_token_details"))
        .and_then(Value::as_object);
    let usage_fields = usage_obj.as_object();

    recognized |= observe_token_field(
        &mut m.evidence.cache_read_input_tokens,
        responses_input_details,
        "cached_tokens",
        UsageEvidenceSource::ResponsesInputTokensDetailsCachedTokens,
    );
    recognized |= observe_token_field(
        &mut m.evidence.cache_read_input_tokens,
        chat_prompt_details,
        "cached_tokens",
        UsageEvidenceSource::ChatPromptTokensDetailsCachedTokens,
    );
    for (field, source) in [
        (
            "cached_input_tokens",
            UsageEvidenceSource::CachedInputTokensAlias,
        ),
        ("cached_tokens", UsageEvidenceSource::CachedTokensAlias),
        (
            "cache_read_input_tokens",
            UsageEvidenceSource::CacheReadInputTokensAlias,
        ),
        (
            "cache_read_tokens",
            UsageEvidenceSource::CacheReadTokensAlias,
        ),
    ] {
        recognized |= observe_token_field(
            &mut m.evidence.cache_read_input_tokens,
            usage_fields,
            field,
            source,
        );
    }
    if let Some(selected) = m.evidence.cache_read_input_tokens.selected() {
        if source_uses_cached_projection(selected.source) {
            m.cached_input_tokens = selected.value;
        } else {
            m.cache_read_input_tokens = selected.value;
        }
    }

    for (object, field, source) in [
        (
            responses_input_details,
            "cache_write_tokens",
            UsageEvidenceSource::ResponsesInputTokensDetailsCacheWriteTokens,
        ),
        (
            chat_prompt_details,
            "cache_write_tokens",
            UsageEvidenceSource::ChatPromptTokensDetailsCacheWriteTokens,
        ),
        (
            responses_input_details,
            "cache_creation_tokens",
            UsageEvidenceSource::ResponsesInputTokensDetailsCacheCreationTokens,
        ),
        (
            chat_prompt_details,
            "cache_creation_tokens",
            UsageEvidenceSource::ChatPromptTokensDetailsCacheCreationTokens,
        ),
    ] {
        recognized |= observe_token_field(
            &mut m.evidence.cache_write_input_tokens,
            object,
            field,
            source,
        );
    }
    for (field, source) in [
        (
            "cache_creation_input_tokens",
            UsageEvidenceSource::CacheCreationInputTokensAlias,
        ),
        (
            "cache_write_input_tokens",
            UsageEvidenceSource::CacheWriteInputTokensAlias,
        ),
        (
            "cache_creation_tokens",
            UsageEvidenceSource::CacheCreationTokensAlias,
        ),
        (
            "cache_write_tokens",
            UsageEvidenceSource::CacheWriteTokensAlias,
        ),
    ] {
        recognized |= observe_token_field(
            &mut m.evidence.cache_write_input_tokens,
            usage_fields,
            field,
            source,
        );
    }

    let cache_creation = usage_obj.get("cache_creation");
    let cache_creation_5m = usage_obj
        .get("cache_creation_5m_input_tokens")
        .or_else(|| cache_creation.and_then(|value| value.get("ephemeral_5m_input_tokens")))
        .or_else(|| usage_obj.get("claude_cache_creation_5_m_tokens"));
    let cache_creation_1h = usage_obj
        .get("cache_creation_1h_input_tokens")
        .or_else(|| cache_creation.and_then(|value| value.get("ephemeral_1h_input_tokens")))
        .or_else(|| usage_obj.get("claude_cache_creation_1_h_tokens"));
    let ttl_present = cache_creation_5m.is_some() || cache_creation_1h.is_some();
    let ttl_5m = cache_creation_5m.and_then(token_count);
    let ttl_1h = cache_creation_1h.and_then(token_count);
    if cache_creation_5m.is_some() {
        recognized = true;
        match ttl_5m {
            Some(value) => m.cache_creation_5m_input_tokens = value,
            None => m
                .evidence
                .cache_write_input_tokens
                .mark_invalid(UsageEvidenceSource::AnthropicCacheCreationTtl),
        }
    }
    if cache_creation_1h.is_some() {
        recognized = true;
        match ttl_1h {
            Some(value) => m.cache_creation_1h_input_tokens = value,
            None => m
                .evidence
                .cache_write_input_tokens
                .mark_invalid(UsageEvidenceSource::AnthropicCacheCreationTtl),
        }
    }
    if ttl_present
        && cache_creation_5m.is_none_or(|_| ttl_5m.is_some())
        && cache_creation_1h.is_none_or(|_| ttl_1h.is_some())
    {
        match ttl_5m
            .unwrap_or_default()
            .checked_add(ttl_1h.unwrap_or_default())
        {
            Some(value) => m
                .evidence
                .cache_write_input_tokens
                .observe_value(UsageEvidenceSource::AnthropicCacheCreationTtl, value),
            None => m
                .evidence
                .cache_write_input_tokens
                .mark_invalid(UsageEvidenceSource::AnthropicCacheCreationTtl),
        }
    }
    if let Some(selected) = m.evidence.cache_write_input_tokens.selected() {
        m.cache_creation_input_tokens = selected.value;
    }

    // If total isn't provided, derive it from input/output when possible.
    if !presence.total_tokens {
        (m.total_tokens, m.total_tokens_source) = m.derived_total();
    }

    if !recognized {
        return None;
    }
    Some(UsageUpdate {
        metrics: m,
        presence,
    })
}

fn merge_usage_update(last: &mut Option<UsageMetrics>, update: UsageUpdate) {
    let UsageUpdate {
        metrics: update,
        presence,
    } = update;
    let Some(existing) = last.as_mut() else {
        *last = Some(update);
        return;
    };
    existing.evidence.aggregate_status = existing
        .evidence
        .economics_status()
        .combine(update.evidence.economics_status());

    if presence.input_tokens {
        existing.input_tokens = update.input_tokens;
    }
    if presence.output_tokens {
        existing.output_tokens = update.output_tokens;
    }
    if presence.reasoning_tokens {
        existing.reasoning_tokens = update.reasoning_tokens;
    }
    if presence.reasoning_output_tokens {
        existing.reasoning_output_tokens = update.reasoning_output_tokens;
    }
    if update.evidence.cache_read_input_tokens.is_present() {
        existing.cached_input_tokens = update.cached_input_tokens;
        existing.cache_read_input_tokens = update.cache_read_input_tokens;
        existing.evidence.cache_read_input_tokens = update.evidence.cache_read_input_tokens;
    } else {
        if update.cached_input_tokens != 0 {
            existing.cached_input_tokens = update.cached_input_tokens;
        }
        if update.cache_read_input_tokens != 0 {
            existing.cache_read_input_tokens = update.cache_read_input_tokens;
        }
    }
    if update.evidence.cache_write_input_tokens.is_present() {
        existing.cache_creation_input_tokens = update.cache_creation_input_tokens;
        existing.cache_creation_5m_input_tokens = update.cache_creation_5m_input_tokens;
        existing.cache_creation_1h_input_tokens = update.cache_creation_1h_input_tokens;
        existing.evidence.cache_write_input_tokens = update.evidence.cache_write_input_tokens;
    } else {
        if update.cache_creation_input_tokens != 0 {
            existing.cache_creation_input_tokens = update.cache_creation_input_tokens;
        }
        if update.cache_creation_5m_input_tokens != 0 {
            existing.cache_creation_5m_input_tokens = update.cache_creation_5m_input_tokens;
        }
        if update.cache_creation_1h_input_tokens != 0 {
            existing.cache_creation_1h_input_tokens = update.cache_creation_1h_input_tokens;
        }
        existing.cache_creation_input_tokens = existing.cache_creation_input_tokens.max(
            existing
                .cache_creation_5m_input_tokens
                .saturating_add(existing.cache_creation_1h_input_tokens),
        );
    }
    if presence.total_tokens {
        existing.total_tokens = update.total_tokens;
        existing.total_tokens_source = UsageTotalSource::Reported;
    } else if existing.total_tokens_source != UsageTotalSource::Reported {
        (existing.total_tokens, existing.total_tokens_source) = existing.derived_total();
    }
}

pub(crate) fn merge_usage_from_json_value(value: &Value, last: &mut Option<UsageMetrics>) {
    if let Some(usage_obj) = extract_usage_obj(value)
        && let Some(update) = usage_update_from_value(usage_obj)
    {
        merge_usage_update(last, update);
    }
}

pub fn extract_usage_from_bytes(data: &[u8]) -> Option<UsageMetrics> {
    let text = std::str::from_utf8(data).ok()?.trim();
    if text.is_empty() {
        return None;
    }
    let json: Value = match serde_json::from_str(text) {
        Ok(json) => json,
        Err(_) => return extract_usage_from_sse_bytes(data),
    };
    let usage_obj = extract_usage_obj(&json)?;
    usage_from_value(usage_obj)
}

pub fn extract_usage_from_sse_bytes(data: &[u8]) -> Option<UsageMetrics> {
    let mut last: Option<UsageMetrics> = None;
    crate::sse::visit_sse_json_values(data, |value| merge_usage_from_json_value(value, &mut last));
    last
}

/// Incrementally scan SSE bytes for `data: {json}` lines that contain usage information.
///
/// This is designed for streaming scenarios where the response arrives in many chunks:
/// it avoids repeatedly re-parsing the entire buffer (which can become O(n^2)).
///
/// - `scan_pos` is an in/out cursor into `data` (byte index).
/// - `last` stores the latest usage parsed so far (updated in-place).
#[cfg(test)]
pub fn scan_usage_from_sse_bytes_incremental(
    data: &[u8],
    scan_pos: &mut usize,
    last: &mut Option<UsageMetrics>,
) {
    let mut i = (*scan_pos).min(data.len());

    while i < data.len() {
        let Some(rel_end) = data[i..].iter().position(|b| *b == b'\n') else {
            break;
        };
        let end = i + rel_end;
        let mut line = &data[i..end];
        i = end.saturating_add(1);

        if line.ends_with(b"\r") {
            line = &line[..line.len().saturating_sub(1)];
        }
        if line.is_empty() {
            continue;
        }

        const DATA_PREFIX: &[u8] = b"data:";
        if !line.starts_with(DATA_PREFIX) {
            continue;
        }
        let mut payload = &line[DATA_PREFIX.len()..];
        while !payload.is_empty() && payload[0].is_ascii_whitespace() {
            payload = &payload[1..];
        }
        if payload.is_empty() || payload == b"[DONE]" {
            continue;
        }

        if let Ok(json) = serde_json::from_slice::<Value>(payload) {
            merge_usage_from_json_value(&json, last);
        }
    }

    *scan_pos = i;
}

#[cfg(test)]
mod tests {
    use super::*;

    use pretty_assertions::assert_eq;

    fn token_evidence(observations: &[(UsageEvidenceSource, i64)]) -> UsageTokenEvidence {
        let mut evidence = UsageTokenEvidence::default();
        for (source, value) in observations {
            evidence.observe_value(*source, *value);
        }
        evidence
    }

    fn cache_evidence(
        read: &[(UsageEvidenceSource, i64)],
        write: &[(UsageEvidenceSource, i64)],
    ) -> UsageEvidence {
        UsageEvidence {
            cache_read_input_tokens: token_evidence(read),
            cache_write_input_tokens: token_evidence(write),
            ..UsageEvidence::default()
        }
    }

    #[test]
    fn incremental_sse_scan_matches_full_parse() {
        let sse = concat!(
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n",
            "\n",
            "event: response.completed\n",
            "data: {\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n",
            "\n"
        );

        let full = extract_usage_from_sse_bytes(sse.as_bytes());
        let mut pos = 0usize;
        let mut last = None;
        scan_usage_from_sse_bytes_incremental(sse.as_bytes(), &mut pos, &mut last);
        assert_eq!(last, full);
    }

    #[test]
    fn incremental_sse_scan_handles_split_lines() {
        let part1 = b"data: {\"response\":{\"usage\":{\"input_tokens\":1";
        let part2 = b",\"output_tokens\":2,\"total_tokens\":3}}}\n\n";
        let mut buf = Vec::new();
        let mut pos = 0usize;
        let mut last = None;

        buf.extend_from_slice(part1);
        scan_usage_from_sse_bytes_incremental(&buf, &mut pos, &mut last);
        assert_eq!(last, None);

        buf.extend_from_slice(part2);
        scan_usage_from_sse_bytes_incremental(&buf, &mut pos, &mut last);
        assert_eq!(
            last,
            Some(UsageMetrics {
                input_tokens: 1,
                output_tokens: 2,
                total_tokens: 3,
                total_tokens_source: UsageTotalSource::Reported,
                ..UsageMetrics::default()
            })
        );
    }

    #[test]
    fn parses_anthropic_message_start_usage() {
        let json = r#"{
          "type":"message_start",
          "message":{
            "usage":{
              "input_tokens":100,
              "cache_read_input_tokens":30,
              "cache_creation":{
                "ephemeral_5m_input_tokens":20,
                "ephemeral_1h_input_tokens":40
              }
            }
          }
        }"#;

        assert_eq!(
            extract_usage_from_bytes(json.as_bytes()),
            Some(UsageMetrics {
                input_tokens: 100,
                cache_read_input_tokens: 30,
                cache_creation_input_tokens: 60,
                cache_creation_5m_input_tokens: 20,
                cache_creation_1h_input_tokens: 40,
                total_tokens: 190,
                evidence: cache_evidence(
                    &[(UsageEvidenceSource::CacheReadInputTokensAlias, 30)],
                    &[(UsageEvidenceSource::AnthropicCacheCreationTtl, 60)],
                ),
                ..UsageMetrics::default()
            })
        );
    }

    #[test]
    fn sse_scan_merges_message_start_and_delta_usage() {
        let sse = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":100,\"cache_read_input_tokens\":30,\"cache_creation\":{\"ephemeral_5m_input_tokens\":20}}}}\n",
            "\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":7}}\n",
            "\n"
        );
        let expected = Some(UsageMetrics {
            input_tokens: 100,
            output_tokens: 7,
            cache_read_input_tokens: 30,
            cache_creation_input_tokens: 20,
            cache_creation_5m_input_tokens: 20,
            total_tokens: 157,
            evidence: cache_evidence(
                &[(UsageEvidenceSource::CacheReadInputTokensAlias, 30)],
                &[(UsageEvidenceSource::AnthropicCacheCreationTtl, 20)],
            ),
            ..UsageMetrics::default()
        });

        assert_eq!(extract_usage_from_sse_bytes(sse.as_bytes()), expected);

        let mut pos = 0usize;
        let mut last = None;
        scan_usage_from_sse_bytes_incremental(sse.as_bytes(), &mut pos, &mut last);
        assert_eq!(last, expected);
    }

    #[test]
    fn extract_usage_from_bytes_falls_back_to_sse_body() {
        let sse = concat!(
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"usage\":{\"input_tokens\":3,\"output_tokens\":4}}\n",
            "\n"
        );

        assert_eq!(
            extract_usage_from_bytes(sse.as_bytes()),
            Some(UsageMetrics {
                input_tokens: 3,
                output_tokens: 4,
                total_tokens: 7,
                ..UsageMetrics::default()
            })
        );
    }

    #[test]
    fn buffered_sse_usage_supports_multiline_data_events() {
        let sse = concat!(
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\n",
            "data: \"response\":{\"usage\":{\"input_tokens\":1000,\n",
            "data: \"output_tokens\":50,\"input_tokens_details\":{\"cached_tokens\":100,\"cache_write_tokens\":200}}}}\n\n",
        );

        let usage = extract_usage_from_sse_bytes(sse.as_bytes()).expect("multiline usage");
        let buckets = usage.canonical_usage_buckets(CacheAccountingConvention::INCLUDED_IN_INPUT);

        assert_eq!(buckets.ordinary_input_tokens, 700);
        assert_eq!(buckets.cache_read_input_tokens, 100);
        assert_eq!(buckets.cache_write_input_tokens, 200);
        assert_eq!(usage.output_tokens, 50);
    }

    #[test]
    fn parses_chat_completions_usage_fields() {
        let json = r#"{
          "id":"chatcmpl_x",
          "object":"chat.completion",
          "usage":{
            "prompt_tokens":9,
            "completion_tokens":12,
            "total_tokens":21,
            "completion_tokens_details":{"reasoning_tokens":5}
          }
        }"#;
        assert_eq!(
            extract_usage_from_bytes(json.as_bytes()),
            Some(UsageMetrics {
                input_tokens: 9,
                output_tokens: 12,
                reasoning_tokens: 5,
                reasoning_output_tokens: 5,
                total_tokens: 21,
                total_tokens_source: UsageTotalSource::Reported,
                ..UsageMetrics::default()
            })
        );
    }

    #[test]
    fn parses_responses_usage_cache_and_reasoning_details() {
        let json = r#"{
          "response":{
            "usage":{
              "input_tokens":100,
              "output_tokens":20,
              "input_tokens_details":{"cached_tokens":40},
              "output_tokens_details":{"reasoning_tokens":7}
            }
          }
        }"#;
        assert_eq!(
            extract_usage_from_bytes(json.as_bytes()),
            Some(UsageMetrics {
                input_tokens: 100,
                output_tokens: 20,
                reasoning_tokens: 7,
                reasoning_output_tokens: 7,
                cached_input_tokens: 40,
                total_tokens: 120,
                evidence: cache_evidence(
                    &[(
                        UsageEvidenceSource::ResponsesInputTokensDetailsCachedTokens,
                        40,
                    )],
                    &[],
                ),
                ..UsageMetrics::default()
            })
        );
    }

    #[test]
    fn responses_nested_cache_write_zero_wins_over_positive_alias_and_keeps_conflict() {
        let json = r#"{
          "usage":{
            "input_tokens":1000,
            "output_tokens":50,
            "total_tokens":1050,
            "input_tokens_details":{
              "cached_tokens":100,
              "cache_write_tokens":0
            },
            "cache_write_tokens":200
          }
        }"#;

        let usage = extract_usage_from_bytes(json.as_bytes()).expect("usage");

        assert_eq!(usage.cache_creation_input_tokens, 0);
        assert_eq!(
            usage.evidence.cache_write_input_tokens.selected(),
            Some(UsageTokenObservation {
                source: UsageEvidenceSource::ResponsesInputTokensDetailsCacheWriteTokens,
                value: 0,
            })
        );
        assert!(usage.evidence.cache_write_input_tokens.has_conflict());
        assert_eq!(
            usage.evidence.cache_write_input_tokens.state(),
            UsageEvidenceState::Conflict
        );
        let serialized = serde_json::to_value(&usage.evidence).expect("serialize evidence");
        assert_eq!(
            serialized["cache_write_input_tokens"]["state"].as_str(),
            Some("conflict")
        );
        assert_eq!(
            usage.cache_hit_rate_with_convention(CacheAccountingConvention::INCLUDED_IN_INPUT),
            None
        );
    }

    #[test]
    fn chat_usage_builds_exclusive_openai_cache_buckets() {
        let json = r#"{
          "usage":{
            "prompt_tokens":1000,
            "completion_tokens":50,
            "total_tokens":1050,
            "prompt_tokens_details":{
              "cached_tokens":100,
              "cache_write_tokens":200
            }
          }
        }"#;

        let usage = extract_usage_from_bytes(json.as_bytes()).expect("usage");
        let buckets = usage.canonical_usage_buckets(CacheAccountingConvention::INCLUDED_IN_INPUT);

        assert_eq!(usage.cached_input_tokens, 100);
        assert_eq!(usage.cache_creation_input_tokens, 200);
        assert_eq!(buckets.ordinary_input_tokens, 700);
        assert_eq!(buckets.cache_read_input_tokens, 100);
        assert_eq!(buckets.cache_write_input_tokens, 200);
        assert_eq!(buckets.status, EconomicsStatus::Complete);
    }

    #[test]
    fn openai_nested_cache_write_does_not_inflate_derived_total() {
        let usage = extract_usage_from_bytes(
            br#"{"usage":{"input_tokens":1000,"output_tokens":50,"input_tokens_details":{"cached_tokens":100,"cache_write_tokens":200}}}"#,
        )
        .expect("usage");

        assert_eq!(usage.total_tokens, 1_050);
    }

    #[test]
    fn ambiguous_root_cache_write_keeps_total_convention_explicit() {
        let usage = extract_usage_from_bytes(
            br#"{"usage":{"input_tokens":1000,"output_tokens":50,"cached_tokens":100,"cache_write_tokens":200}}"#,
        )
        .expect("usage");

        assert_eq!(usage.total_tokens, 1_050);
        assert_eq!(
            usage.total_tokens_source,
            UsageTotalSource::DerivedWithoutConvention
        );
        assert_eq!(
            usage
                .canonical_usage_buckets(CacheAccountingConvention::INCLUDED_IN_INPUT)
                .ordinary_input_tokens,
            700
        );
    }

    #[test]
    fn usage_evidence_round_trips_without_losing_source_or_status() {
        let usage = extract_usage_from_bytes(
            br#"{"usage":{"input_tokens":1000,"input_tokens_details":{"cached_tokens":100,"cache_write_tokens":200}}}"#,
        )
        .expect("usage");
        let encoded = serde_json::to_vec(&usage).expect("serialize usage");
        let decoded: UsageMetrics = serde_json::from_slice(&encoded).expect("deserialize usage");

        assert_eq!(decoded, usage);
        assert_eq!(
            decoded
                .canonical_usage_buckets(CacheAccountingConvention::INCLUDED_IN_INPUT)
                .status,
            EconomicsStatus::Complete
        );
    }

    #[test]
    fn invalid_deserialized_evidence_cannot_produce_complete_economics() {
        let usage: UsageMetrics = serde_json::from_value(serde_json::json!({
            "input_tokens": 100,
            "evidence": {
                "cache_read_input_tokens": {},
                "cache_write_input_tokens": {
                    "selected": {"source": "cache_write_tokens_alias", "value": -5},
                    "observations": [
                        {"source": "cache_write_tokens_alias", "value": -5}
                    ]
                }
            }
        }))
        .expect("deserialize usage");

        assert_eq!(
            usage
                .canonical_usage_buckets(CacheAccountingConvention::INCLUDED_IN_INPUT)
                .status,
            EconomicsStatus::Conflict
        );
    }

    #[test]
    fn overflowing_cache_buckets_are_conflicting_economics() {
        let usage = UsageMetrics {
            input_tokens: i64::MAX,
            cache_read_input_tokens: i64::MAX,
            cache_creation_input_tokens: i64::MAX,
            ..UsageMetrics::default()
        };

        let buckets = usage.canonical_usage_buckets(CacheAccountingConvention::INCLUDED_IN_INPUT);

        assert_eq!(buckets.ordinary_input_tokens, 0);
        assert_eq!(buckets.status, EconomicsStatus::Conflict);
    }

    #[test]
    fn responses_and_chat_usage_produce_identical_openai_cache_buckets() {
        let responses = extract_usage_from_bytes(
            br#"{"usage":{"input_tokens":1000,"output_tokens":50,"total_tokens":1050,"input_tokens_details":{"cached_tokens":100,"cache_write_tokens":200}}}"#,
        )
        .expect("responses usage");
        let chat = extract_usage_from_bytes(
            br#"{"usage":{"prompt_tokens":1000,"completion_tokens":50,"total_tokens":1050,"prompt_tokens_details":{"cached_tokens":100,"cache_write_tokens":200}}}"#,
        )
        .expect("chat usage");

        let responses_buckets =
            responses.canonical_usage_buckets(CacheAccountingConvention::INCLUDED_IN_INPUT);
        let chat_buckets =
            chat.canonical_usage_buckets(CacheAccountingConvention::INCLUDED_IN_INPUT);

        assert_eq!(responses_buckets, chat_buckets);
        assert_eq!(
            responses_buckets,
            CanonicalUsageBuckets {
                ordinary_input_tokens: 700,
                cache_read_input_tokens: 100,
                cache_write_input_tokens: 200,
                status: EconomicsStatus::Complete,
            }
        );
    }

    #[test]
    fn cache_write_aliases_preserve_value_and_source() {
        for (field, source) in [
            (
                "cache_creation_input_tokens",
                UsageEvidenceSource::CacheCreationInputTokensAlias,
            ),
            (
                "cache_write_input_tokens",
                UsageEvidenceSource::CacheWriteInputTokensAlias,
            ),
            (
                "cache_creation_tokens",
                UsageEvidenceSource::CacheCreationTokensAlias,
            ),
            (
                "cache_write_tokens",
                UsageEvidenceSource::CacheWriteTokensAlias,
            ),
        ] {
            let payload = serde_json::json!({"usage": {"input_tokens": 100, field: 17}});
            let bytes = serde_json::to_vec(&payload).expect("serialize fixture");
            let usage = extract_usage_from_bytes(&bytes).expect("usage");

            assert_eq!(usage.cache_creation_input_tokens, 17, "field: {field}");
            assert_eq!(
                usage.evidence.cache_write_input_tokens.selected(),
                Some(UsageTokenObservation { source, value: 17 }),
                "field: {field}"
            );
            assert_eq!(
                usage.evidence.cache_write_input_tokens.state(),
                UsageEvidenceState::PresentValue,
                "field: {field}"
            );
        }
    }

    #[test]
    fn cache_write_evidence_distinguishes_missing_zero_and_value() {
        let missing = extract_usage_from_bytes(br#"{"usage":{"input_tokens":10}}"#)
            .expect("missing cache usage");
        let zero = extract_usage_from_bytes(
            br#"{"usage":{"input_tokens":10,"cache_write_input_tokens":0}}"#,
        )
        .expect("zero cache usage");
        let value = extract_usage_from_bytes(
            br#"{"usage":{"input_tokens":10,"cache_write_input_tokens":5}}"#,
        )
        .expect("value cache usage");

        assert_eq!(
            missing.evidence.cache_write_input_tokens.state(),
            UsageEvidenceState::Missing
        );
        assert_eq!(
            zero.evidence.cache_write_input_tokens.state(),
            UsageEvidenceState::PresentZero
        );
        assert_eq!(
            value.evidence.cache_write_input_tokens.state(),
            UsageEvidenceState::PresentValue
        );
    }

    #[test]
    fn explicit_separate_convention_keeps_nested_cache_outside_ordinary_input() {
        let json = r#"{
          "usage":{
            "input_tokens":1000,
            "output_tokens":50,
            "input_tokens_details":{"cached_tokens":100,"cache_write_tokens":200}
          }
        }"#;

        let usage = extract_usage_from_bytes(json.as_bytes()).expect("usage");
        let buckets = usage.canonical_usage_buckets(CacheAccountingConvention::SEPARATE);

        assert_eq!(buckets.ordinary_input_tokens, 1000);
        assert_eq!(buckets.cache_read_input_tokens, 100);
        assert_eq!(buckets.cache_write_input_tokens, 200);
        assert_eq!(buckets.status, EconomicsStatus::Complete);
    }

    #[test]
    fn sse_explicit_cache_write_zero_replaces_an_earlier_positive_value() {
        let sse = concat!(
            "data: {\"usage\":{\"input_tokens\":1000,\"cache_write_tokens\":200}}\n\n",
            "data: {\"usage\":{\"input_tokens\":1000,\"input_tokens_details\":{\"cache_write_tokens\":0}}}\n\n"
        );

        let usage = extract_usage_from_sse_bytes(sse.as_bytes()).expect("usage");
        let mut scan_pos = 0;
        let mut incremental = None;
        scan_usage_from_sse_bytes_incremental(sse.as_bytes(), &mut scan_pos, &mut incremental);

        assert_eq!(usage.cache_creation_input_tokens, 0);
        assert_eq!(
            usage.evidence.cache_write_input_tokens.state(),
            UsageEvidenceState::PresentZero
        );
        assert_eq!(incremental, Some(usage));
    }

    #[test]
    fn sse_cache_economics_conflict_remains_after_a_clean_snapshot() {
        for first_chunk in [
            "data: {\"usage\":{\"input_tokens\":1000,\"output_tokens\":50,\"input_tokens_details\":{\"cached_tokens\":100,\"cache_write_tokens\":0},\"cache_write_tokens\":200}}\n\n",
            "data: {\"usage\":{\"input_tokens\":1000,\"output_tokens\":50,\"input_tokens_details\":{\"cached_tokens\":100,\"cache_write_tokens\":\"invalid\"}}}\n\n",
        ] {
            let sse = format!(
                "{first_chunk}data: {{\"usage\":{{\"input_tokens\":1000,\"output_tokens\":50,\"input_tokens_details\":{{\"cached_tokens\":100,\"cache_write_tokens\":0}}}}}}\n\n"
            );

            let full = extract_usage_from_sse_bytes(sse.as_bytes()).expect("full usage");
            let mut scan_pos = 0;
            let mut incremental = None;
            scan_usage_from_sse_bytes_incremental(sse.as_bytes(), &mut scan_pos, &mut incremental);

            for usage in [&full, incremental.as_ref().expect("incremental usage")] {
                assert_eq!(usage.cache_creation_input_tokens, 0);
                assert_eq!(
                    usage.evidence.cache_write_input_tokens.state(),
                    UsageEvidenceState::PresentZero
                );
                assert_eq!(usage.evidence.aggregate_status, EconomicsStatus::Conflict);
                assert_eq!(
                    usage
                        .canonical_usage_buckets(CacheAccountingConvention::INCLUDED_IN_INPUT)
                        .status,
                    EconomicsStatus::Conflict
                );
            }
            assert_eq!(incremental, Some(full));
        }
    }

    #[test]
    fn sse_explicit_cache_read_zero_replaces_an_earlier_positive_value() {
        let sse = concat!(
            "data: {\"usage\":{\"input_tokens\":1000,\"cached_tokens\":100}}\n\n",
            "data: {\"usage\":{\"input_tokens\":1000,\"input_tokens_details\":{\"cached_tokens\":0}}}\n\n"
        );

        let usage = extract_usage_from_sse_bytes(sse.as_bytes()).expect("usage");
        let mut scan_pos = 0;
        let mut incremental = None;
        scan_usage_from_sse_bytes_incremental(sse.as_bytes(), &mut scan_pos, &mut incremental);

        assert_eq!(usage.cached_input_tokens, 0);
        assert_eq!(usage.cache_read_input_tokens, 0);
        assert_eq!(
            usage.evidence.cache_read_input_tokens.state(),
            UsageEvidenceState::PresentZero
        );
        assert_eq!(incremental, Some(usage));
    }

    #[test]
    fn sse_explicit_zero_replaces_base_token_counts() {
        let sse = concat!(
            "data: {\"usage\":{\"input_tokens\":10,\"output_tokens\":7,\"reasoning_tokens\":5}}\n\n",
            "data: {\"usage\":{\"input_tokens\":0,\"output_tokens\":0,\"reasoning_tokens\":0}}\n\n"
        );

        let usage = extract_usage_from_sse_bytes(sse.as_bytes()).expect("usage");
        let mut scan_pos = 0;
        let mut incremental = None;
        scan_usage_from_sse_bytes_incremental(sse.as_bytes(), &mut scan_pos, &mut incremental);

        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.reasoning_tokens, 0);
        assert_eq!(usage.reasoning_output_tokens, 0);
        assert_eq!(usage.total_tokens, 0);
        assert_eq!(incremental, Some(usage));
    }

    #[test]
    fn sse_keeps_a_reported_total_when_a_later_chunk_omits_it() {
        let sse = concat!(
            "data: {\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"total_tokens\":15}}\n\n",
            "data: {\"usage\":{\"output_tokens\":7}}\n\n"
        );

        let usage = extract_usage_from_sse_bytes(sse.as_bytes()).expect("usage");

        assert_eq!(usage.output_tokens, 7);
        assert_eq!(usage.total_tokens, 15);
    }

    #[test]
    fn aggregate_preserves_conflicting_usage_evidence() {
        let conflicting = extract_usage_from_bytes(
            br#"{"usage":{"input_tokens":1000,"input_tokens_details":{"cache_write_tokens":0},"cache_write_tokens":200}}"#,
        )
        .expect("usage");
        let mut aggregate = UsageMetrics::default();

        aggregate.add_assign(&conflicting);

        assert_eq!(
            aggregate
                .canonical_usage_buckets(CacheAccountingConvention::INCLUDED_IN_INPUT)
                .status,
            EconomicsStatus::Conflict
        );
    }

    #[test]
    fn aggregate_marks_each_token_bucket_overflow_as_conflicting_economics() {
        macro_rules! assert_overflow_conflict {
            ($field:ident, $left:expr, $right:expr, $expected:expr) => {{
                let mut aggregate = UsageMetrics {
                    $field: $left,
                    ..UsageMetrics::default()
                };
                let increment = UsageMetrics {
                    $field: $right,
                    ..UsageMetrics::default()
                };

                aggregate.add_assign(&increment);

                assert_eq!(aggregate.$field, $expected, stringify!($field));
                assert_eq!(
                    aggregate.evidence.aggregate_status,
                    EconomicsStatus::Conflict,
                    stringify!($field)
                );
                assert_eq!(
                    aggregate
                        .canonical_usage_buckets(CacheAccountingConvention::SEPARATE)
                        .status,
                    EconomicsStatus::Conflict,
                    stringify!($field)
                );
            }};
        }

        assert_overflow_conflict!(input_tokens, i64::MAX, 1, i64::MAX);
        assert_overflow_conflict!(output_tokens, i64::MAX, 1, i64::MAX);
        assert_overflow_conflict!(reasoning_tokens, i64::MAX, 1, i64::MAX);
        assert_overflow_conflict!(reasoning_output_tokens, i64::MAX, 1, i64::MAX);
        assert_overflow_conflict!(total_tokens, i64::MAX, 1, i64::MAX);
        assert_overflow_conflict!(cached_input_tokens, i64::MAX, 1, i64::MAX);
        assert_overflow_conflict!(cache_read_input_tokens, i64::MAX, 1, i64::MAX);
        assert_overflow_conflict!(cache_creation_input_tokens, i64::MAX, 1, i64::MAX);
        assert_overflow_conflict!(cache_creation_5m_input_tokens, i64::MAX, 1, i64::MAX);
        assert_overflow_conflict!(cache_creation_1h_input_tokens, i64::MAX, 1, i64::MAX);
        assert_overflow_conflict!(input_tokens, i64::MIN, -1, i64::MIN);
    }

    #[test]
    fn invalid_and_negative_main_token_fields_are_conflicting_economics() {
        for payload in [
            br#"{"usage":{"input_tokens":"not-a-token-count","output_tokens":1}}"#.as_slice(),
            br#"{"usage":{"input_tokens":"1.5","output_tokens":1}}"#.as_slice(),
            br#"{"usage":{"input_tokens":1.5,"output_tokens":1}}"#.as_slice(),
            br#"{"usage":{"input_tokens":1,"output_tokens":-1}}"#.as_slice(),
        ] {
            let usage = extract_usage_from_bytes(payload).expect("recognized usage");

            assert_eq!(
                usage
                    .canonical_usage_buckets(CacheAccountingConvention::SEPARATE)
                    .status,
                EconomicsStatus::Conflict,
                "{}",
                String::from_utf8_lossy(payload)
            );
        }

        let directly_constructed = UsageMetrics {
            input_tokens: 1,
            output_tokens: -1,
            ..UsageMetrics::default()
        };
        assert_eq!(
            directly_constructed
                .canonical_usage_buckets(CacheAccountingConvention::SEPARATE)
                .status,
            EconomicsStatus::Conflict
        );
    }

    #[test]
    fn canonical_main_token_fields_win_when_chat_aliases_conflict() {
        for payload in [
            br#"{"usage":{"input_tokens":10,"prompt_tokens":11,"output_tokens":5,"completion_tokens":5}}"#.as_slice(),
            br#"{"usage":{"input_tokens":10,"prompt_tokens":10,"output_tokens":5,"completion_tokens":6}}"#.as_slice(),
        ] {
            let usage = extract_usage_from_bytes(payload).expect("recognized usage");

            assert_eq!(usage.input_tokens, 10);
            assert_eq!(usage.output_tokens, 5);
            assert_eq!(usage.evidence.aggregate_status, EconomicsStatus::Conflict);
            assert_eq!(
                usage
                    .canonical_usage_buckets(CacheAccountingConvention::SEPARATE)
                    .status,
                EconomicsStatus::Conflict
            );
        }
    }

    #[test]
    fn matching_canonical_main_token_fields_and_chat_aliases_are_complete() {
        let usage = extract_usage_from_bytes(
            br#"{"usage":{"input_tokens":10,"prompt_tokens":10,"output_tokens":5,"completion_tokens":5}}"#,
        )
        .expect("recognized usage");

        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(usage.evidence.aggregate_status, EconomicsStatus::Complete);
        assert_eq!(
            usage
                .canonical_usage_buckets(CacheAccountingConvention::SEPARATE)
                .status,
            EconomicsStatus::Complete
        );
    }

    #[test]
    fn main_token_arithmetic_and_reported_total_contradictions_are_conflicting() {
        let overflowing = UsageMetrics {
            input_tokens: i64::MAX,
            output_tokens: 1,
            ..UsageMetrics::default()
        };
        assert_eq!(
            overflowing
                .canonical_usage_buckets(CacheAccountingConvention::SEPARATE)
                .status,
            EconomicsStatus::Conflict
        );

        for reported_total in [14, 16] {
            let usage = extract_usage_from_bytes(
                format!(
                    "{{\"usage\":{{\"input_tokens\":10,\"output_tokens\":5,\"total_tokens\":{reported_total}}}}}"
                )
                .as_bytes(),
            )
            .expect("recognized usage");

            assert_eq!(
                usage
                    .canonical_usage_buckets(CacheAccountingConvention::SEPARATE)
                    .status,
                EconomicsStatus::Conflict,
                "reported total: {reported_total}"
            );
        }
    }

    #[test]
    fn invalid_cache_write_evidence_is_not_coerced_to_zero() {
        let json = r#"{
          "usage":{
            "input_tokens":1000,
            "input_tokens_details":{"cache_write_tokens":"not-a-token-count"}
          }
        }"#;

        let usage = extract_usage_from_bytes(json.as_bytes()).expect("usage");

        assert_eq!(
            usage.evidence.cache_write_input_tokens.state(),
            UsageEvidenceState::Invalid
        );
        assert_eq!(
            usage
                .canonical_usage_buckets(CacheAccountingConvention::INCLUDED_IN_INPUT)
                .status,
            EconomicsStatus::Conflict
        );
    }

    #[test]
    fn included_cache_totals_that_exceed_input_are_conflicting_economics() {
        let usage = UsageMetrics {
            input_tokens: 100,
            cache_read_input_tokens: 80,
            cache_creation_input_tokens: 40,
            ..UsageMetrics::default()
        };

        let buckets = usage.canonical_usage_buckets(CacheAccountingConvention::INCLUDED_IN_INPUT);

        assert_eq!(buckets.ordinary_input_tokens, 0);
        assert_eq!(buckets.status, EconomicsStatus::Conflict);
    }

    #[test]
    fn missing_cache_convention_is_partial_instead_of_guessing_from_service() {
        let usage = UsageMetrics {
            input_tokens: 1_000,
            cache_read_input_tokens: 100,
            cache_creation_input_tokens: 200,
            ..UsageMetrics::default()
        };

        let buckets = usage.canonical_usage_buckets(CacheAccountingConvention::UNKNOWN);

        assert_eq!(buckets.ordinary_input_tokens, 1_000);
        assert_eq!(buckets.cache_read_input_tokens, 100);
        assert_eq!(buckets.cache_write_input_tokens, 200);
        assert_eq!(buckets.status, EconomicsStatus::Partial);
    }

    #[test]
    fn parses_openai_cached_tokens_before_direct_cache_read_tokens() {
        let json = r#"{
          "usage":{
            "input_tokens":100,
            "output_tokens":20,
            "input_tokens_details":{"cached_tokens":40},
            "cache_read_input_tokens":30
          }
        }"#;
        assert_eq!(
            extract_usage_from_bytes(json.as_bytes()),
            Some(UsageMetrics {
                input_tokens: 100,
                output_tokens: 20,
                cached_input_tokens: 40,
                total_tokens: 120,
                evidence: cache_evidence(
                    &[
                        (
                            UsageEvidenceSource::ResponsesInputTokensDetailsCachedTokens,
                            40,
                        ),
                        (UsageEvidenceSource::CacheReadInputTokensAlias, 30),
                    ],
                    &[],
                ),
                ..UsageMetrics::default()
            })
        );
    }

    #[test]
    fn parses_anthropic_cache_usage_fields() {
        let json = r#"{
          "usage":{
            "input_tokens":10,
            "output_tokens":5,
            "cache_read_input_tokens":30,
            "cache_creation_5m_input_tokens":20,
            "cache_creation_1h_input_tokens":40
          }
        }"#;
        assert_eq!(
            extract_usage_from_bytes(json.as_bytes()),
            Some(UsageMetrics {
                input_tokens: 10,
                output_tokens: 5,
                total_tokens: 105,
                cache_read_input_tokens: 30,
                cache_creation_input_tokens: 60,
                cache_creation_5m_input_tokens: 20,
                cache_creation_1h_input_tokens: 40,
                evidence: cache_evidence(
                    &[(UsageEvidenceSource::CacheReadInputTokensAlias, 30)],
                    &[(UsageEvidenceSource::AnthropicCacheCreationTtl, 60)],
                ),
                ..UsageMetrics::default()
            })
        );
    }

    #[test]
    fn parses_cache_creation_aliases_used_by_relay_bridges() {
        let json = r#"{
          "usage":{
            "input_tokens":10,
            "output_tokens":5,
            "cached_tokens":4,
            "cache_creation":{
              "ephemeral_5m_input_tokens":20,
              "ephemeral_1h_input_tokens":40
            }
          }
        }"#;
        assert_eq!(
            extract_usage_from_bytes(json.as_bytes()),
            Some(UsageMetrics {
                input_tokens: 10,
                output_tokens: 5,
                cached_input_tokens: 4,
                total_tokens: 75,
                cache_creation_input_tokens: 60,
                cache_creation_5m_input_tokens: 20,
                cache_creation_1h_input_tokens: 40,
                evidence: cache_evidence(
                    &[(UsageEvidenceSource::CachedTokensAlias, 4)],
                    &[(UsageEvidenceSource::AnthropicCacheCreationTtl, 60)],
                ),
                ..UsageMetrics::default()
            })
        );
    }

    #[test]
    fn parses_claude_cache_creation_aliases() {
        let json = r#"{
          "usage":{
            "input_tokens":10,
            "output_tokens":5,
            "claude_cache_creation_5_m_tokens":20,
            "claude_cache_creation_1_h_tokens":40
          }
        }"#;
        assert_eq!(
            extract_usage_from_bytes(json.as_bytes()),
            Some(UsageMetrics {
                input_tokens: 10,
                output_tokens: 5,
                total_tokens: 75,
                cache_creation_input_tokens: 60,
                cache_creation_5m_input_tokens: 20,
                cache_creation_1h_input_tokens: 40,
                evidence: cache_evidence(
                    &[],
                    &[(UsageEvidenceSource::AnthropicCacheCreationTtl, 60)],
                ),
                ..UsageMetrics::default()
            })
        );
    }

    #[test]
    fn computes_cache_hit_rate_from_read_and_creation_tokens() {
        let usage = UsageMetrics {
            input_tokens: 100,
            cache_read_input_tokens: 30,
            cache_creation_input_tokens: 20,
            ..UsageMetrics::default()
        };

        let rate = usage
            .cache_hit_rate_with_convention(CacheAccountingConvention::SEPARATE)
            .expect("cache hit rate");

        assert_eq!(rate, 0.2);
    }

    #[test]
    fn computes_cache_hit_rate_when_direct_cache_read_is_included_in_input() {
        let usage = UsageMetrics {
            input_tokens: 100,
            cache_read_input_tokens: 30,
            cache_creation_input_tokens: 20,
            ..UsageMetrics::default()
        };

        let rate = usage
            .cache_hit_rate_with_convention(CacheAccountingConvention {
                cache_read: CacheTokenInclusion::IncludedInInput,
                cache_write: CacheTokenInclusion::Separate,
            })
            .expect("cache hit rate");

        assert_eq!(rate, 0.25);
    }

    #[test]
    fn computes_cache_hit_rate_from_cached_input_tokens() {
        let usage = UsageMetrics {
            input_tokens: 100,
            cached_input_tokens: 40,
            ..UsageMetrics::default()
        };

        let rate = usage
            .cache_hit_rate_with_convention(CacheAccountingConvention::INCLUDED_IN_INPUT)
            .expect("cache hit rate");

        assert_eq!(rate, 0.4);
    }

    #[test]
    fn computes_cache_hit_rate_from_mixed_usage_cache_fields() {
        let usage = UsageMetrics {
            input_tokens: 1_500,
            cached_input_tokens: 50,
            cache_read_input_tokens: 250,
            ..UsageMetrics::default()
        };

        let rate = usage
            .cache_hit_rate_with_convention(CacheAccountingConvention::INCLUDED_IN_INPUT)
            .expect("cache hit rate");

        assert!((rate - (300.0 / 1_500.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn cache_hit_rate_is_controlled_by_captured_convention_not_service_name() {
        let usage = UsageMetrics {
            input_tokens: 1_500,
            cached_input_tokens: 50,
            cache_read_input_tokens: 250,
            ..UsageMetrics::default()
        };

        let included_rate = usage
            .cache_hit_rate_with_convention(CacheAccountingConvention::INCLUDED_IN_INPUT)
            .expect("included cache hit rate");
        let separate_rate = usage
            .cache_hit_rate_with_convention(CacheAccountingConvention::SEPARATE)
            .expect("cache hit rate");

        assert!((included_rate - (300.0 / 1_500.0)).abs() < f64::EPSILON);
        assert!((separate_rate - (300.0 / 1_800.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn unknown_usage_schema_returns_none() {
        let json = r#"{"usage":{"foo":123}}"#;
        assert_eq!(extract_usage_from_bytes(json.as_bytes()), None);
    }
}
