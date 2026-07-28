use sysinfo::System;

use crate::domain::{
    AdaptiveModelProfile, AdaptiveProfileId, CapabilityState, CapabilityStatus, HostCapabilities,
    ModelState, ModelStatus,
};

pub fn detect_host(model: &ModelStatus) -> HostCapabilities {
    let mut system = System::new_all();
    system.refresh_memory();
    system.refresh_cpu_all();
    let total_memory_gb = system.total_memory() as f64 / 1_073_741_824.0;
    let available_memory_gb = system.available_memory() as f64 / 1_073_741_824.0;
    let logical_cpu_count = system.cpus().len();
    HostCapabilities {
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        total_memory_gb: round_one_decimal(total_memory_gb),
        available_memory_gb: round_one_decimal(available_memory_gb),
        logical_cpu_count,
        gpu: unknown("No portable runtime-qualified accelerator probe has run."),
        battery: unknown("Power state is unavailable; heavy work remains conservative."),
        metered_network: unknown("Network cost is unknown; Web never downloads models."),
        local_runtime: CapabilityStatus {
            state: match model.state {
                ModelState::Ready => CapabilityState::Available,
                ModelState::RuntimeUnavailable | ModelState::ModelMissing => {
                    CapabilityState::Unavailable
                }
                ModelState::Incompatible | ModelState::Degraded => CapabilityState::Degraded,
                _ => CapabilityState::Unknown,
            },
            detail: model.detail.clone(),
        },
        recommended_profile: recommend_profile(
            total_memory_gb,
            available_memory_gb,
            logical_cpu_count,
            model,
        ),
    }
}

pub fn recommend_profile(
    total_memory_gb: f64,
    available_memory_gb: f64,
    logical_cpu_count: usize,
    model: &ModelStatus,
) -> AdaptiveModelProfile {
    let ready_name = (model.state == ModelState::Ready)
        .then_some(model.model.as_deref())
        .flatten();
    let selected = ready_name.map_or_else(
        || "No behaviorally ready model selected; extractive fallback".to_owned(),
        |name| format!("Selected ready model: {name}"),
    );
    if ready_name.is_some()
        && total_memory_gb >= 32.0
        && available_memory_gb >= 20.0
        && logical_cpu_count >= 12
    {
        AdaptiveModelProfile {
            id: AdaptiveProfileId::Performance,
            title: "Performance host envelope".into(),
            generation_model: selected,
            embedding_model: "Disabled until separately measured".into(),
            context_window: 16_384,
            max_concurrent_requests: 1,
            rationale: "The selected exact model passed a structured probe and current memory headroom supports one bounded request. This does not infer model size or GPU support.".into(),
            requires_explicit_download: true,
        }
    } else if ready_name.is_some()
        && total_memory_gb >= 12.0
        && available_memory_gb >= 8.0
        && logical_cpu_count >= 6
    {
        AdaptiveModelProfile {
            id: AdaptiveProfileId::Balanced,
            title: "Balanced host envelope".into(),
            generation_model: selected,
            embedding_model: "Disabled until separately measured".into(),
            context_window: 8_192,
            max_concurrent_requests: 1,
            rationale: "The selected exact model passed a structured probe and current memory headroom supports one bounded request. Model parameter and quantization facts come only from Ollama metadata.".into(),
            requires_explicit_download: true,
        }
    } else {
        AdaptiveModelProfile {
            id: AdaptiveProfileId::CpuBasic,
            title: "CPU / basic host envelope".into(),
            generation_model: selected,
            embedding_model: "Disabled until a separate local capability probe passes".into(),
            context_window: 4_096,
            max_concurrent_requests: 1,
            rationale: "Unknown, unmeasured, unavailable, or memory-constrained systems stay conservative. The recommendation never selects or downloads a model.".into(),
            requires_explicit_download: true,
        }
    }
}

fn unknown(detail: &str) -> CapabilityStatus {
    CapabilityStatus {
        state: CapabilityState::Unknown,
        detail: detail.into(),
    }
}
fn round_one_decimal(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference::fallback_status;

    fn ready(name: &str) -> ModelStatus {
        let mut status = fallback_status("test");
        status.state = ModelState::Ready;
        status.model = Some(name.into());
        status.structured_output = true;
        status
    }

    #[test]
    fn unknown_or_unmeasured_hosts_never_elevate_or_infer_model_size() {
        let unavailable = fallback_status("test");
        assert_eq!(
            recommend_profile(64.0, 48.0, 16, &unavailable).id,
            AdaptiveProfileId::CpuBasic
        );
        let profile = recommend_profile(64.0, 48.0, 16, &ready("exact:8b"));
        assert_eq!(profile.id, AdaptiveProfileId::Performance);
        assert!(profile.generation_model.contains("exact:8b"));
        assert!(!profile.generation_model.contains("12–14B"));
    }

    #[test]
    fn renderer_capability_grants_no_unused_core_defaults() {
        let capability = include_str!("../capabilities/main.json");
        assert!(!capability.contains("core:default"));
        let parsed: serde_json::Value = serde_json::from_str(capability).expect("capability json");
        assert_eq!(
            parsed
                .get("permissions")
                .and_then(|value| value.as_array())
                .map(Vec::len),
            Some(0)
        );
    }

    #[test]
    fn profiles_require_available_headroom_align_context_and_use_one_request() {
        let model = ready("exact:7b");
        let constrained = recommend_profile(64.0, 4.0, 16, &model);
        let balanced = recommend_profile(16.0, 10.0, 8, &model);
        let performance = recommend_profile(64.0, 40.0, 16, &model);
        assert_eq!(constrained.context_window, 4_096);
        assert_eq!(balanced.context_window, 8_192);
        assert_eq!(performance.context_window, 16_384);
        for profile in [constrained, balanced, performance] {
            assert_eq!(profile.max_concurrent_requests, 1);
            assert!(profile.requires_explicit_download);
        }
    }
}
