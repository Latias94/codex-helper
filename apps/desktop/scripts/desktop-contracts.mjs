import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import ts from "typescript";

export const desktopRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repositoryRoot = path.resolve(desktopRoot, "../..");
const rustSchemaManifest = path.join(
  repositoryRoot,
  "tools/desktop-contract-schema/Cargo.toml",
);

const contractDefinitions = [
  {
    output: "src/generated/admin-read-model.contract.json",
    contract: "codex-helper-desktop-admin-read-model/v1",
    version: 1,
    rust: [
      {
        id: "adminReadModel",
        file: "src-tauri/src/commands/admin_api.rs",
        struct: "AdminReadModel",
        shape: true,
      },
      {
        id: "operatorReadModel",
        file: "../../crates/core/src/dashboard_core/operator_summary.rs",
        struct: "OperatorReadModel",
        shape: true,
      },
      {
        id: "operatorRevisionBundle",
        file: "../../crates/core/src/dashboard_core/operator_summary.rs",
        struct: "OperatorRevisionBundle",
        shape: true,
      },
      {
        id: "operatorReadData",
        file: "../../crates/core/src/dashboard_core/operator_summary.rs",
        struct: "OperatorReadData",
        shape: true,
      },
      {
        id: "operatorSummary",
        file: "../../crates/core/src/dashboard_core/operator_summary.rs",
        struct: "ApiV1OperatorSummary",
        shape: true,
      },
      {
        id: "operatorSummaryCounts",
        file: "../../crates/core/src/dashboard_core/operator_summary.rs",
        struct: "OperatorSummaryCounts",
        shape: true,
      },
      {
        id: "operatorRetrySummary",
        file: "../../crates/core/src/dashboard_core/operator_summary.rs",
        struct: "OperatorRetrySummary",
        shape: true,
      },
      {
        id: "operatorActionCapabilities",
        file: "../../crates/core/src/dashboard_core/operator_summary.rs",
        struct: "OperatorActionCapabilities",
        shape: true,
      },
      {
        id: "operatorRoutingSummary",
        file: "../../crates/core/src/dashboard_core/operator_summary.rs",
        struct: "OperatorRoutingSummary",
        shape: true,
      },
      {
        id: "operatorRouteTargetSummary",
        file: "../../crates/core/src/dashboard_core/operator_summary.rs",
        struct: "OperatorRouteTargetSummary",
        shape: true,
      },
      {
        id: "operatorRouteCandidateSummary",
        file: "../../crates/core/src/dashboard_core/operator_summary.rs",
        struct: "OperatorRouteCandidateSummary",
        shape: true,
      },
      {
        id: "controlProfileOption",
        file: "../../crates/core/src/dashboard_core/types.rs",
        struct: "ControlProfileOption",
        shape: true,
      },
      {
        id: "operatorProviderSummary",
        file: "../../crates/core/src/dashboard_core/operator_summary.rs",
        struct: "OperatorProviderSummary",
        shape: true,
      },
      {
        id: "operatorProviderEndpointSummary",
        file: "../../crates/core/src/dashboard_core/operator_summary.rs",
        struct: "OperatorProviderEndpointSummary",
        shape: true,
      },
      {
        id: "credentialReadinessDetail",
        file: "../../crates/core/src/credentials/model.rs",
        struct: "CredentialReadinessDetail",
        shape: true,
      },
      {
        id: "operatorProviderCapacity",
        file: "../../crates/core/src/dashboard_core/operator_summary.rs",
        struct: "OperatorProviderCapacity",
        shape: true,
      },
      {
        id: "operatorPolicyActionSummary",
        file: "../../crates/core/src/dashboard_core/operator_summary.rs",
        struct: "OperatorPolicyActionSummary",
        shape: true,
      },
      {
        id: "operatorSessionSummary",
        file: "../../crates/core/src/dashboard_core/operator_summary.rs",
        struct: "OperatorSessionSummary",
        shape: true,
      },
      {
        id: "operatorSessionRouteAffinitySummary",
        file: "../../crates/core/src/dashboard_core/operator_summary.rs",
        struct: "OperatorSessionRouteAffinitySummary",
        shape: true,
      },
      {
        id: "routeDecisionProvenance",
        file: "../../crates/core/src/state/session_identity.rs",
        struct: "RouteDecisionProvenance",
        shape: true,
      },
      {
        id: "operatorActiveRequestSummary",
        file: "../../crates/core/src/dashboard_core/operator_summary.rs",
        struct: "OperatorActiveRequestSummary",
        shape: true,
      },
      {
        id: "operatorRequestSummary",
        file: "../../crates/core/src/dashboard_core/operator_summary.rs",
        struct: "OperatorRequestSummary",
        shape: true,
      },
      {
        id: "operatorRetrySummaryView",
        file: "../../crates/core/src/dashboard_core/operator_summary.rs",
        struct: "OperatorRetrySummaryView",
        shape: true,
      },
      {
        id: "operatorRouteAttemptSummary",
        file: "../../crates/core/src/dashboard_core/operator_summary.rs",
        struct: "OperatorRouteAttemptSummary",
        shape: true,
      },
      {
        id: "operatorRequestObservability",
        file: "../../crates/core/src/dashboard_core/operator_summary.rs",
        struct: "OperatorRequestObservability",
        shape: true,
      },
      {
        id: "operatorProviderBalanceSummary",
        file: "../../crates/core/src/dashboard_core/operator_summary.rs",
        struct: "OperatorProviderBalanceSummary",
        shape: true,
      },
      {
        id: "providerUsageWindow",
        file: "../../crates/core/src/balance.rs",
        struct: "ProviderUsageWindow",
        shape: true,
      },
      {
        id: "providerUsageRateSnapshot",
        file: "../../crates/core/src/balance.rs",
        struct: "ProviderUsageRateSnapshot",
        shape: true,
      },
      {
        id: "providerUsageModelStat",
        file: "../../crates/core/src/balance.rs",
        struct: "ProviderUsageModelStat",
        shape: true,
      },
      {
        id: "operatorRuntimeSummary",
        file: "../../crates/core/src/dashboard_core/operator_summary.rs",
        struct: "OperatorRuntimeSummary",
        shape: true,
      },
      {
        id: "operatorProfileSummary",
        file: "../../crates/core/src/dashboard_core/operator_summary.rs",
        struct: "OperatorProfileSummary",
        shape: true,
      },
      {
        id: "usageDayCoverage",
        file: "../../crates/core/src/state/runtime_types.rs",
        struct: "UsageDayCoverage",
        shape: true,
      },
      {
        id: "requestUsageAggregate",
        file: "../../crates/core/src/request_ledger.rs",
        struct: "RequestUsageAggregate",
        shape: true,
      },
      {
        id: "requestUsageSummaryRow",
        file: "../../crates/core/src/request_ledger.rs",
        struct: "RequestUsageSummaryRow",
        shape: true,
      },
      {
        id: "requestUsageSummaryCoverage",
        file: "../../crates/core/src/request_ledger.rs",
        struct: "RequestUsageSummaryCoverage",
        shape: true,
      },
      {
        id: "requestUsageSummary",
        file: "../../crates/core/src/request_ledger.rs",
        struct: "RequestUsageSummary",
        shape: true,
      },
      {
        id: "usageMetrics",
        file: "../../crates/core/src/usage.rs",
        struct: "UsageMetrics",
        shape: true,
      },
      {
        id: "usageEvidence",
        file: "../../crates/core/src/usage.rs",
        struct: "UsageEvidence",
        shape: true,
      },
      {
        id: "usageTokenEvidence",
        file: "../../crates/core/src/usage.rs",
        struct: "UsageTokenEvidenceWire",
        shape: true,
      },
      {
        id: "usageTokenObservation",
        file: "../../crates/core/src/usage.rs",
        struct: "UsageTokenObservation",
        shape: true,
      },
      {
        id: "costBreakdown",
        file: "../../crates/core/src/pricing.rs",
        struct: "CostBreakdown",
        shape: true,
      },
      {
        id: "resolvedRouteValue",
        file: "../../crates/core/src/state/session_identity.rs",
        struct: "ResolvedRouteValue",
        shape: true,
      },
      {
        id: "usageBucket",
        file: "../../crates/core/src/state/runtime_types.rs",
        struct: "UsageBucket",
        shape: true,
      },
      {
        id: "usageCostSummary",
        file: "../../crates/core/src/pricing.rs",
        struct: "CostSummary",
        shape: true,
      },
      {
        id: "usageDayHourRow",
        file: "../../crates/core/src/state/runtime_types.rs",
        struct: "UsageDayHourRow",
        shape: true,
      },
      {
        id: "usageDayDimensionRow",
        file: "../../crates/core/src/state/runtime_types.rs",
        struct: "UsageDayDimensionRow",
        shape: true,
      },
      {
        id: "usageRetryGateReasonRow",
        file: "../../crates/core/src/state/runtime_types.rs",
        struct: "UsageRetryGateReasonRow",
        shape: true,
      },
      {
        id: "usageRetryGateSummary",
        file: "../../crates/core/src/state/runtime_types.rs",
        struct: "UsageRetryGateSummary",
        shape: true,
      },
      {
        id: "usageDayView",
        file: "../../crates/core/src/state/runtime_types.rs",
        struct: "UsageDayView",
        shape: true,
      },
      {
        id: "usageRollupCoverage",
        file: "../../crates/core/src/state/runtime_types.rs",
        struct: "UsageRollupCoverage",
        shape: true,
      },
      {
        id: "usageRollupView",
        file: "../../crates/core/src/state/runtime_types.rs",
        struct: "UsageRollupView",
        shape: true,
      },
      {
        id: "windowStats",
        file: "../../crates/core/src/dashboard_core/window_stats.rs",
        struct: "WindowStats",
        shape: true,
      },
      {
        id: "modelPriceView",
        file: "../../crates/core/src/pricing.rs",
        struct: "ModelPriceView",
        shape: true,
      },
      {
        id: "modelPriceCatalogSnapshot",
        file: "../../crates/core/src/pricing.rs",
        struct: "ModelPriceCatalogSnapshot",
        shape: true,
      },
      {
        id: "quotaQuantity",
        file: "../../crates/core/src/quota_pool.rs",
        struct: "QuotaQuantity",
        shape: true,
      },
    ],
    enums: [
      {
        file: "../../crates/core/src/credentials/model.rs",
        rust: "CredentialReadinessCode",
        typescriptFile: "src/lib/api/admin-types.ts",
        typescript: "ApiCredentialReadinessCode",
        rename: "snake",
      },
      {
        file: "../../crates/core/src/credentials/model.rs",
        rust: "CredentialBindingKind",
        typescriptFile: "src/lib/api/admin-types.ts",
        typescript: "ApiCredentialBindingKind",
        rename: "snake",
      },
      {
        file: "../../crates/core/src/credentials/model.rs",
        rust: "CredentialAggregateReadiness",
        typescriptFile: "src/lib/api/admin-types.ts",
        typescript: "ApiCredentialAggregateReadiness",
        rename: "snake",
      },
      {
        file: "../../crates/core/src/config.rs",
        rust: "RouteStrategy",
        typescriptFile: "src/lib/api/admin-types.ts",
        typescript: "ApiRouteStrategy",
        rename: "kebab",
      },
      {
        file: "../../crates/core/src/config.rs",
        rust: "RouteAffinityPolicy",
        typescriptFile: "src/lib/api/admin-types.ts",
        typescript: "ApiRouteAffinityPolicy",
        rename: "kebab",
      },
      {
        file: "../../crates/core/src/config.rs",
        rust: "SchedulingPreset",
        typescriptFile: "src/lib/api/admin-types.ts",
        typescript: "ApiSchedulingPreset",
        rename: "kebab",
      },
      {
        file: "../../crates/core/src/dashboard_core/operator_summary.rs",
        rust: "OperatorReadStatus",
        typescriptFile: "src/lib/api/admin-types.ts",
        typescript: "ApiOperatorReadStatus",
        rename: "snake",
      },
      {
        file: "../../crates/core/src/dashboard_core/operator_summary.rs",
        rust: "OperatorReadIssue",
        typescriptFile: "src/lib/api/admin-types.ts",
        typescript: "ApiOperatorReadIssue",
        rename: "snake",
      },
      {
        file: "../../crates/core/src/state/session_identity.rs",
        rust: "SessionContinuityMode",
        typescriptFile: "src/lib/api/admin-types.ts",
        typescript: "ApiSessionContinuityMode",
        rename: "snake",
      },
      {
        file: "../../crates/core/src/balance.rs",
        rust: "BalanceSnapshotStatus",
        typescriptFile: "src/lib/api/admin-types.ts",
        typescript: "ApiBalanceSnapshotStatus",
        rename: "snake",
      },
      {
        file: "../../crates/core/src/balance.rs",
        rust: "ProviderUsageAlertKind",
        typescriptFile: "src/lib/api/admin-types.ts",
        typescript: "ApiProviderUsageAlertKind",
        rename: "snake",
      },
      {
        file: "../../crates/core/src/config_retry.rs",
        rust: "RetryProfileName",
        typescriptFile: "src/lib/api/admin-types.ts",
        typescript: "ApiRetryProfileName",
        rename: "kebab",
      },
      {
        file: "../../crates/core/src/request_ledger.rs",
        rust: "RequestUsageSummaryGroup",
        typescriptFile: "src/lib/api/admin-types.ts",
        typescript: "ApiRequestUsageSummaryGroup",
        rename: "snake",
      },
      {
        file: "../../crates/core/src/usage.rs",
        rust: "UsageEvidenceSource",
        typescriptFile: "src/lib/api/admin-types.ts",
        typescript: "ApiUsageEvidenceSource",
      },
      {
        file: "../../crates/core/src/usage.rs",
        rust: "UsageEvidenceState",
        typescriptFile: "src/lib/api/admin-types.ts",
        typescript: "ApiUsageEvidenceState",
      },
      {
        file: "../../crates/core/src/usage.rs",
        rust: "EconomicsStatus",
        typescriptFile: "src/lib/api/admin-types.ts",
        typescript: "ApiEconomicsStatus",
      },
      {
        file: "../../crates/core/src/usage.rs",
        rust: "UsageTotalSource",
        typescriptFile: "src/lib/api/admin-types.ts",
        typescript: "ApiUsageTotalSource",
      },
      {
        file: "../../crates/core/src/pricing.rs",
        rust: "CostConfidence",
        typescriptFile: "src/lib/api/admin-types.ts",
        typescript: "ApiCostConfidence",
      },
      {
        file: "../../crates/core/src/state/runtime_types.rs",
        rust: "RuntimeConfigState",
        typescriptFile: "src/lib/api/admin-types.ts",
        typescript: "ApiRuntimeConfigState",
      },
      {
        file: "../../crates/core/src/state/session_identity.rs",
        rust: "RouteValueSource",
        typescriptFile: "src/lib/api/admin-types.ts",
        typescript: "ApiRouteValueSource",
      },
    ],
    typescript: [
      {
        file: "src/lib/tauri/commands.ts",
        type: "AdminReadModel",
        fieldsFrom: "adminReadModel",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiOperatorReadModelWire",
        fieldsFrom: "operatorReadModel",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiOperatorRevisionBundle",
        fieldsFrom: "operatorRevisionBundle",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiOperatorReadData",
        fieldsFrom: "operatorReadData",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiOperatorSummary",
        fieldsFrom: "operatorSummary",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiOperatorSummaryCounts",
        fieldsFrom: "operatorSummaryCounts",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiOperatorRetrySummary",
        fieldsFrom: "operatorRetrySummary",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiOperatorActionCapabilities",
        fieldsFrom: "operatorActionCapabilities",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiOperatorRoutingSummary",
        fieldsFrom: "operatorRoutingSummary",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiOperatorRouteTargetSummary",
        fieldsFrom: "operatorRouteTargetSummary",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiOperatorRouteCandidateSummary",
        fieldsFrom: "operatorRouteCandidateSummary",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiControlProfileOption",
        fieldsFrom: "controlProfileOption",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiOperatorProviderSummary",
        fieldsFrom: "operatorProviderSummary",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiOperatorProviderEndpointSummary",
        fieldsFrom: "operatorProviderEndpointSummary",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiCredentialReadinessDetail",
        fieldsFrom: "credentialReadinessDetail",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiOperatorProviderCapacity",
        fieldsFrom: "operatorProviderCapacity",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiOperatorPolicyActionSummary",
        fieldsFrom: "operatorPolicyActionSummary",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiOperatorSessionSummary",
        fieldsFrom: "operatorSessionSummary",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiOperatorSessionRouteAffinitySummary",
        fieldsFrom: "operatorSessionRouteAffinitySummary",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiOperatorRouteDecision",
        fieldsFrom: "routeDecisionProvenance",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiOperatorActiveRequestSummary",
        fieldsFrom: "operatorActiveRequestSummary",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiOperatorRequestSummary",
        fieldsFrom: "operatorRequestSummary",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiOperatorRetrySummaryView",
        fieldsFrom: "operatorRetrySummaryView",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiOperatorRouteAttemptSummary",
        fieldsFrom: "operatorRouteAttemptSummary",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiOperatorRequestObservability",
        fieldsFrom: "operatorRequestObservability",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiOperatorProviderBalanceSummary",
        fieldsFrom: "operatorProviderBalanceSummary",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiProviderUsageWindow",
        fieldsFrom: "providerUsageWindow",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiProviderUsageRateSnapshot",
        fieldsFrom: "providerUsageRateSnapshot",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiProviderUsageModelStat",
        fieldsFrom: "providerUsageModelStat",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiOperatorRuntimeSummary",
        fieldsFrom: "operatorRuntimeSummary",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiOperatorProfileSummary",
        fieldsFrom: "operatorProfileSummary",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiUsageDayCoverage",
        fieldsFrom: "usageDayCoverage",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiRequestUsageAggregate",
        fieldsFrom: "requestUsageAggregate",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiRequestUsageSummaryRow",
        fieldsFrom: "requestUsageSummaryRow",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiRequestUsageSummaryCoverage",
        fieldsFrom: "requestUsageSummaryCoverage",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiRequestUsageSummary",
        fieldsFrom: "requestUsageSummary",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiUsageMetrics",
        fieldsFrom: "usageMetrics",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiUsageEvidence",
        fieldsFrom: "usageEvidence",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiUsageTokenEvidence",
        fieldsFrom: "usageTokenEvidence",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiUsageTokenObservation",
        fieldsFrom: "usageTokenObservation",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiCostBreakdown",
        fieldsFrom: "costBreakdown",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiResolvedRouteValue",
        fieldsFrom: "resolvedRouteValue",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiUsageBucket",
        fieldsFrom: "usageBucket",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiUsageCostSummary",
        fieldsFrom: "usageCostSummary",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiUsageDayHourRow",
        fieldsFrom: "usageDayHourRow",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiUsageDayDimensionRow",
        fieldsFrom: "usageDayDimensionRow",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiUsageRetryGateReasonRow",
        fieldsFrom: "usageRetryGateReasonRow",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiUsageRetryGateSummary",
        fieldsFrom: "usageRetryGateSummary",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiUsageDayView",
        fieldsFrom: "usageDayView",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiUsageRollupCoverage",
        fieldsFrom: "usageRollupCoverage",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiUsageRollupView",
        fieldsFrom: "usageRollupView",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiWindowStats",
        fieldsFrom: "windowStats",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiModelPriceView",
        fieldsFrom: "modelPriceView",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiModelPriceCatalogSnapshot",
        fieldsFrom: "modelPriceCatalogSnapshot",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiQuotaQuantity",
        fieldsFrom: "quotaQuantity",
        strictShape: true,
      },
    ],
    notes: [
      "Generated from the desktop wrapper and the canonical core OperatorReadModel DTO.",
      "Nested status enums, revision fields, read-data fields, requiredness, and scalar/reference types are checked.",
      "Request-chain diagnostics retain a separate allowlisted contract.",
    ],
  },
  {
    output: "src/generated/request-chain.contract.json",
    contract: "codex-helper-request-chain/v1",
    version: 1,
    rust: [
      {
        id: "requestChainSelector",
        file: "../../crates/core/src/request_chain.rs",
        struct: "RequestChainSelector",
        shape: true,
      },
      {
        id: "requestChainExport",
        file: "../../crates/core/src/request_chain.rs",
        struct: "RequestChainExport",
        shape: true,
      },
      {
        id: "requestChainRequest",
        file: "../../crates/core/src/request_chain.rs",
        struct: "RequestChainRequest",
        shape: true,
      },
      {
        id: "requestChainRouteAttempt",
        file: "../../crates/core/src/request_chain.rs",
        struct: "RequestChainRouteAttempt",
        shape: true,
      },
      {
        id: "requestChainProviderSignal",
        file: "../../crates/core/src/request_chain.rs",
        struct: "RequestChainProviderSignal",
        shape: true,
      },
      {
        id: "requestChainPolicyAction",
        file: "../../crates/core/src/request_chain.rs",
        struct: "RequestChainPolicyAction",
        shape: true,
      },
      {
        id: "requestChainTimelineEvent",
        file: "../../crates/core/src/request_chain.rs",
        struct: "RequestChainTimelineEvent",
        shape: true,
      },
      {
        id: "requestObservability",
        file: "../../crates/core/src/state/session_identity.rs",
        struct: "RequestObservability",
        shape: true,
      },
      {
        id: "providerEndpointKey",
        file: "../../crates/core/src/runtime_identity.rs",
        struct: "ProviderEndpointKey",
        shape: true,
      },
      {
        id: "requestChainPayload",
        file: "src-tauri/src/commands/admin_api.rs",
        struct: "RequestChainPayload",
        shape: true,
        deserializeInput: true,
      },
    ],
    enums: [
      {
        file: "../../crates/core/src/state/session_identity.rs",
        rust: "SessionIdentitySource",
        typescriptFile: "src/lib/api/admin-types.ts",
        typescript: "ApiSessionIdentitySource",
      },
      {
        file: "../../crates/core/src/provider_signals/model.rs",
        rust: "ProviderSignalKind",
        typescriptFile: "src/lib/api/admin-types.ts",
        typescript: "ApiProviderSignalKind",
      },
      {
        file: "../../crates/core/src/provider_signals/model.rs",
        rust: "ProviderSignalSource",
        typescriptFile: "src/lib/api/admin-types.ts",
        typescript: "ApiProviderSignalSource",
      },
      {
        file: "../../crates/core/src/provider_signals/model.rs",
        rust: "ProviderSignalConfidence",
        typescriptFile: "src/lib/api/admin-types.ts",
        typescript: "ApiProviderSignalConfidence",
      },
      {
        file: "../../crates/core/src/policy_actions/model.rs",
        rust: "PolicyActionKind",
        typescriptFile: "src/lib/api/admin-types.ts",
        typescript: "ApiPolicyActionKind",
      },
      {
        file: "../../crates/core/src/policy_actions/model.rs",
        rust: "PolicyActionOwner",
        typescriptFile: "src/lib/api/admin-types.ts",
        typescript: "ApiPolicyActionOwner",
      },
      {
        file: "../../crates/core/src/policy_actions/model.rs",
        rust: "PolicyActionRecoveryState",
        typescriptFile: "src/lib/api/admin-types.ts",
        typescript: "ApiPolicyActionRecoveryState",
      },
    ],
    typescript: [
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiRequestChainSelector",
        fieldsFrom: "requestChainSelector",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiRequestChainExport",
        fieldsFrom: "requestChainExport",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiRequestChainRequest",
        fieldsFrom: "requestChainRequest",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiRequestChainRouteAttempt",
        fieldsFrom: "requestChainRouteAttempt",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiRequestChainProviderSignal",
        fieldsFrom: "requestChainProviderSignal",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiRequestChainPolicyAction",
        fieldsFrom: "requestChainPolicyAction",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiRequestChainTimelineEvent",
        fieldsFrom: "requestChainTimelineEvent",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiRequestObservability",
        fieldsFrom: "requestObservability",
        strictShape: true,
      },
      {
        file: "src/lib/api/admin-types.ts",
        type: "ApiProviderEndpointKey",
        fieldsFrom: "providerEndpointKey",
        strictShape: true,
      },
      {
        file: "src/lib/tauri/commands.ts",
        type: "RequestChainPayload",
        fieldsFrom: "requestChainPayload",
        transform: "camel",
        strictShape: true,
      },
    ],
    notes: [
      "Generated by apps/desktop/scripts/generate-desktop-contracts.mjs from Rust DTO field declarations.",
      "Request-chain export is an allowlisted diagnostic DTO. It must not be replaced by raw FinishedRequest or raw request-log JSON.",
      "The desktop bridge fetches this DTO on demand; the read model does not batch-load request chains.",
    ],
  },
];

export function buildDesktopContracts() {
  const rustSchemas = extractRustSchemas(contractDefinitions);
  return contractDefinitions.map((definition) => {
    const rustTargets = definition.rust.map((target) => {
      const schema = rustSchemas.get(rustSchemaKey("struct", target.file, target.struct));
      if (!schema?.fields) {
        throw new Error(`Missing structured Rust schema for ${target.file}:${target.struct}`);
      }
      return {
        file: target.file,
        struct: target.struct,
        fields: schema.fields.map((field) => field.name),
        shape: target.shape
          ? schema.fields.map((field) => ({
              name: field.name,
              optional:
                Boolean(field.optional) ||
                Boolean(target.deserializeInput && field.wire_type.kind === "nullable"),
              type: rustWireTypeToTypescript(
                target.deserializeInput && field.wire_type.kind === "nullable"
                  ? field.wire_type.inner
                  : field.wire_type,
              ),
            }))
          : undefined,
        id: target.id,
      };
    });
    const fieldsById = new Map(rustTargets.map((target) => [target.id, target.fields]));

    const contract = {
      contract: definition.contract,
      version: definition.version,
      rust: rustTargets.map(({ id: _id, ...target }) => target),
      typescript: definition.typescript.map((target) => ({
        file: target.file,
        type: target.type,
        fields: fieldsFromTarget(fieldsById, target),
        ...(target.strictShape
          ? { shape: shapeFromTarget(rustTargets, target) }
          : {}),
      })),
    };

    if (definition.enums) {
      contract.enums = definition.enums.map((target) => {
        const schema = rustSchemas.get(rustSchemaKey("enum", target.file, target.rust));
        if (!schema?.variants) {
          throw new Error(`Missing structured Rust schema for ${target.file}:${target.rust}`);
        }
        return {
          file: target.file,
          rust: target.rust,
          typescriptFile: target.typescriptFile,
          typescript: target.typescript,
          values: schema.variants,
        };
      });
    }

    if (definition.notes) {
      contract.notes = definition.notes;
    }

    return {
      output: definition.output,
      contract,
    };
  });
}

export function contractOutputPath(output) {
  return path.join(desktopRoot, output);
}

export function formatContractJson(contract) {
  return `${JSON.stringify(contract, null, 2)}\n`;
}

export function readDesktopFile(relativePath) {
  return fs.readFileSync(path.resolve(desktopRoot, relativePath), "utf8");
}

export function parseTypescriptObjectFields(relativePath, typeName) {
  return parseTypescriptObjectShape(relativePath, typeName).map((field) => field.name);
}

export function parseTypescriptObjectShape(relativePath, typeName) {
  const source = readDesktopFile(relativePath);
  return parseTypescriptObjectShapeFromSource(source, typeName, relativePath);
}

export function parseTypescriptObjectShapeFromSource(source, typeName, label = "<memory>") {
  const declaration = findTypescriptTypeAlias(source, typeName, label);
  if (!ts.isTypeLiteralNode(declaration.type)) {
    throw new Error(`${label}:${typeName} must be a direct object type literal`);
  }
  const shape = declaration.type.members.map((member) => {
    if (!ts.isPropertySignature(member) || !member.type || !member.name) {
      throw new Error(`${label}:${typeName} contains an unsupported object member`);
    }
    return {
      name: typescriptPropertyName(member.name, label, typeName),
      optional: Boolean(member.questionToken),
      type: typescriptTypeToContract(member.type, label, typeName),
    };
  });
  if (shape.length === 0) {
    throw new Error(`${label}:${typeName} has no fields`);
  }
  return shape;
}

export function typescriptObjectShapeFailures(actualShape, expectedShape, label) {
  const failures = [];
  if (actualShape.length !== expectedShape.length) {
    failures.push(
      `${label} shape has ${actualShape.length} fields, expected ${expectedShape.length}`,
    );
  }
  const expectedByName = new Map(expectedShape.map((field) => [field.name, field]));
  for (const actual of actualShape) {
    const expected = expectedByName.get(actual.name);
    if (!expected) {
      continue;
    }
    if (actual.optional !== expected.optional) {
      failures.push(
        `${label}.${actual.name} optional=${actual.optional}, expected ${expected.optional}`,
      );
    }
    if (actual.type !== expected.type) {
      failures.push(`${label}.${actual.name} type ${actual.type}, expected ${expected.type}`);
    }
  }
  const actualByName = new Map(actualShape.map((field) => [field.name, field]));
  for (const expected of expectedShape) {
    if (!actualByName.has(expected.name)) {
      failures.push(`${label} shape missing field ${expected.name}`);
    }
  }
  return failures;
}

export function parseTypescriptStringUnion(relativePath, typeName) {
  const source = readDesktopFile(relativePath);
  return parseTypescriptStringUnionFromSource(source, typeName, relativePath);
}

export function parseTypescriptStringUnionFromSource(source, typeName, label = "<memory>") {
  const declaration = findTypescriptTypeAlias(source, typeName, label);
  const members = ts.isUnionTypeNode(declaration.type)
    ? declaration.type.types
    : [declaration.type];
  return members.map((member) => {
    if (!ts.isLiteralTypeNode(member) || !ts.isStringLiteral(member.literal)) {
      throw new Error(`${label}:${typeName} must contain only string literal members`);
    }
    return member.literal.text;
  });
}

function fieldsFromTarget(fieldsById, target) {
  const fields = target.fields ?? fieldsById.get(target.fieldsFrom);
  if (!fields) {
    throw new Error(`Unknown fields source ${target.fieldsFrom} for ${target.type}`);
  }
  if (target.transform === "camel") {
    return fields.map(toCamelCase);
  }
  return [...fields];
}

function shapeFromTarget(rustTargets, target) {
  const source = rustTargets.find((candidate) => candidate.id === target.fieldsFrom);
  if (!source?.shape) {
    throw new Error(`Unknown shape source ${target.fieldsFrom} for ${target.type}`);
  }
  return source.shape.map((field) => ({
    ...field,
    name: target.transform === "camel" ? toCamelCase(field.name) : field.name,
  }));
}

function rustWireTypeToTypescript(wireType) {
  if (wireType.kind === "number" || wireType.kind === "string" || wireType.kind === "boolean") {
    return wireType.kind;
  }
  if (wireType.kind === "array") {
    const element = rustWireTypeToTypescript(wireType.element);
    return `${element.includes("|") ? `(${element})` : element}[]`;
  }
  if (wireType.kind === "tuple") {
    return `[${wireType.elements.map(rustWireTypeToTypescript).join(",")}]`;
  }
  if (wireType.kind === "map") {
    return `Record<${rustWireTypeToTypescript(wireType.key)},${rustWireTypeToTypescript(wireType.value)}>`;
  }
  if (wireType.kind === "nullable") {
    return [rustWireTypeToTypescript(wireType.inner), "null"].sort().join("|");
  }
  if (wireType.kind !== "reference") {
    throw new Error(`Unsupported Rust wire type ${JSON.stringify(wireType)}`);
  }
  const mappings = {
    ApiV1OperatorSummary: "ApiOperatorSummary",
    AdminEndpointConfig: "AdminEndpointConfig",
    CostSummary: "ApiUsageCostSummary",
    OperatorReadModel: "ApiOperatorReadModel",
    RouteDecisionProvenance: "ApiOperatorRouteDecision",
    UsageTokenEvidenceWire: "ApiUsageTokenEvidence",
  };
  return mappings[wireType.name] ?? `Api${wireType.name}`;
}

function extractRustSchemas(definitions) {
  const requests = new Map();
  for (const definition of definitions) {
    for (const target of definition.rust) {
      const key = rustSchemaKey("struct", target.file, target.struct);
      requests.set(key, {
        id: key,
        file: path.resolve(desktopRoot, target.file),
        item: target.struct,
        kind: "struct",
      });
    }
    for (const target of definition.enums ?? []) {
      const key = rustSchemaKey("enum", target.file, target.rust);
      requests.set(key, {
        id: key,
        file: path.resolve(desktopRoot, target.file),
        item: target.rust,
        kind: "enum",
      });
    }
  }
  const cargo = process.env.CARGO ?? "cargo";
  const targetDirectory =
    process.env.CODEX_HELPER_CONTRACT_TARGET_DIR ??
    path.join(os.tmpdir(), `codex-helper-desktop-contract-schema-${process.getuid?.() ?? "user"}`);
  const result = spawnSync(
    cargo,
    ["run", "--quiet", "--locked", "--manifest-path", rustSchemaManifest],
    {
      cwd: repositoryRoot,
      encoding: "utf8",
      env: {
        ...process.env,
        CARGO_INCREMENTAL: "0",
        CARGO_TARGET_DIR: targetDirectory,
      },
      input: JSON.stringify({ targets: [...requests.values()] }),
      maxBuffer: 16 * 1024 * 1024,
    },
  );
  if (result.error) {
    throw new Error(`Failed to start Rust contract schema extractor: ${result.error.message}`);
  }
  if (result.status !== 0) {
    throw new Error(
      `Rust contract schema extractor failed with status ${result.status}:\n${result.stderr.trim()}`,
    );
  }
  let response;
  try {
    response = JSON.parse(result.stdout);
  } catch (error) {
    throw new Error(`Rust contract schema extractor returned invalid JSON: ${error.message}`);
  }
  return new Map(response.targets.map((target) => [target.id, target]));
}

function rustSchemaKey(kind, file, item) {
  return `${kind}:${file}:${item}`;
}

function findTypescriptTypeAlias(source, typeName, label) {
  const sourceFile = ts.createSourceFile(label, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  if (sourceFile.parseDiagnostics.length > 0) {
    const diagnostic = sourceFile.parseDiagnostics[0];
    throw new Error(`${label}: invalid TypeScript: ${ts.flattenDiagnosticMessageText(diagnostic.messageText, "\n")}`);
  }
  const matches = sourceFile.statements.filter(
    (statement) => ts.isTypeAliasDeclaration(statement) && statement.name.text === typeName,
  );
  if (matches.length !== 1) {
    throw new Error(`${label}:${typeName} expected one type alias, found ${matches.length}`);
  }
  return matches[0];
}

function typescriptPropertyName(name, label, typeName) {
  if (ts.isIdentifier(name) || ts.isStringLiteral(name) || ts.isNumericLiteral(name)) {
    return name.text;
  }
  throw new Error(`${label}:${typeName} contains an unsupported computed property name`);
}

function typescriptTypeToContract(node, label, typeName) {
  if (node.kind === ts.SyntaxKind.NumberKeyword) return "number";
  if (node.kind === ts.SyntaxKind.StringKeyword) return "string";
  if (node.kind === ts.SyntaxKind.BooleanKeyword) return "boolean";
  if (node.kind === ts.SyntaxKind.NullKeyword) return "null";
  if (ts.isParenthesizedTypeNode(node)) {
    return typescriptTypeToContract(node.type, label, typeName);
  }
  if (ts.isArrayTypeNode(node)) {
    const element = typescriptTypeToContract(node.elementType, label, typeName);
    return `${element.includes("|") ? `(${element})` : element}[]`;
  }
  if (ts.isTupleTypeNode(node)) {
    return `[${node.elements
      .map((element) => typescriptTypeToContract(element, label, typeName))
      .join(",")}]`;
  }
  if (ts.isUnionTypeNode(node)) {
    return node.types
      .map((member) => typescriptTypeToContract(member, label, typeName))
      .sort()
      .join("|");
  }
  if (ts.isLiteralTypeNode(node)) {
    if (node.literal.kind === ts.SyntaxKind.NullKeyword) return "null";
    if (ts.isStringLiteral(node.literal)) return JSON.stringify(node.literal.text);
    if (ts.isNumericLiteral(node.literal)) return node.literal.text;
  }
  if (ts.isTypeReferenceNode(node)) {
    const reference = typescriptEntityName(node.typeName);
    if (reference === "Array" || reference === "ReadonlyArray") {
      if (node.typeArguments?.length !== 1) {
        throw new Error(`${label}:${typeName} ${reference} requires one type argument`);
      }
      const element = typescriptTypeToContract(node.typeArguments[0], label, typeName);
      return `${element.includes("|") ? `(${element})` : element}[]`;
    }
    if (reference === "Record") {
      if (node.typeArguments?.length !== 2) {
        throw new Error(`${label}:${typeName} Record requires two type arguments`);
      }
      const key = typescriptTypeToContract(node.typeArguments[0], label, typeName);
      const value = typescriptTypeToContract(node.typeArguments[1], label, typeName);
      return `Record<${key},${value}>`;
    }
    if (node.typeArguments && node.typeArguments.length > 0) {
      throw new Error(`${label}:${typeName} uses unsupported generic type ${reference}`);
    }
    return reference;
  }
  throw new Error(
    `${label}:${typeName} uses unsupported TypeScript type ${ts.SyntaxKind[node.kind]}`,
  );
}

function typescriptEntityName(name) {
  if (ts.isIdentifier(name)) {
    return name.text;
  }
  return `${typescriptEntityName(name.left)}.${name.right.text}`;
}

function toCamelCase(value) {
  return value.replace(/_([a-z0-9])/g, (_match, char) => char.toUpperCase());
}
