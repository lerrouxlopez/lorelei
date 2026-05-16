#![forbid(unsafe_code)]

/// Golden test categories covered by `lorelei-eval`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoldenCategory {
    ManualPearlSaveRetrieve,
    EchoRetrievesRelevant,
    EchoNoCrossTenant,
    DeletedPearlExcluded,
    PreferenceAffectsAnswer,
    UnsupportedProviderFails,
    ProviderFallbackWorks,
    InvalidPlannerJsonRepairedOnce,
    HighRiskRequiresApproval,
    LowRiskRunsAutomatically,
    ReflectionStoresAndRejects,
    DockerConfigValidates,
    HarborHealthReady,
    NoSecretsLoggedByDefault,
}
