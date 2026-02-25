#[cfg(test)]
mod tests {
    use serde::Serialize;
    use super::*;
    use crate::Validate;
    use crate::{from_cbor, to_cbor};
    use uuid::Uuid;

    fn sample_request_id() -> Uuid {
        Uuid::from_u128(0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA)
    }

    fn sample_deploy_prepare_request() -> DeployPrepareRequest {
        DeployPrepareRequest {
            name: "syslog-forwarder".to_string(),
            app_type: "socket".to_string(),
            target: StringMap::new(),
            artifact_digest: "sha256:1111".to_string(),
            artifact_size: 1024,
            manifest_digest: "sha256:2222".to_string(),
            idempotency_key: "deploy-1".to_string(),
            policy: StringMap::new(),
        }
    }

    #[test]
    fn hello_negotiate_round_trip_and_validate() {
        let request = HelloNegotiateRequest {
            client_version: "0.1.0".to_string(),
            required_features: vec!["resumable-upload".to_string()],
        };

        request.validate().expect("request should be valid");
        let encoded = to_cbor(&request).expect("encoding should succeed");
        let decoded: HelloNegotiateRequest = from_cbor(&encoded).expect("decoding should succeed");
        assert_eq!(decoded, request);
    }

    #[derive(Debug, Serialize)]
    struct HelloNegotiateMissingRequiredFeatures<'a> {
        client_version: &'a str,
    }

    #[test]
    fn hello_negotiate_rejects_missing_required_field() {
        let encoded = to_cbor(&HelloNegotiateMissingRequiredFeatures {
            client_version: "0.1.0",
        })
        .expect("encoding should succeed");

        let decoded = from_cbor::<HelloNegotiateRequest>(&encoded);
        assert!(decoded.is_err());
    }

    #[derive(Debug, Serialize)]
    struct HelloNegotiateWithLegacyField<'a> {
        client_version: &'a str,
        required_features: Vec<&'a str>,
        compatibility_date: &'a str,
    }

    #[test]
    fn hello_negotiate_rejects_legacy_compatibility_date_field() {
        let encoded = to_cbor(&HelloNegotiateWithLegacyField {
            client_version: "0.1.0",
            required_features: vec![],
            compatibility_date: "2026-02-10",
        })
        .expect("encoding should succeed");

        let decoded = from_cbor::<HelloNegotiateRequest>(&encoded);
        assert!(decoded.is_err());
    }

    #[derive(Debug, Serialize)]
    struct DeployPrepareMissingIdempotency<'a> {
        name: &'a str,
        #[serde(rename = "type")]
        app_type: &'a str,
        target: StringMap,
        artifact_digest: &'a str,
        artifact_size: u64,
        manifest_digest: &'a str,
        policy: StringMap,
    }

    #[test]
    fn deploy_prepare_rejects_missing_idempotency_key() {
        let encoded = to_cbor(&DeployPrepareMissingIdempotency {
            name: "syslog-forwarder",
            app_type: "socket",
            target: StringMap::new(),
            artifact_digest: "sha256:1111",
            artifact_size: 2048,
            manifest_digest: "sha256:2222",
            policy: StringMap::new(),
        })
        .expect("encoding should succeed");

        let decoded = from_cbor::<DeployPrepareRequest>(&encoded);
        assert!(decoded.is_err());
    }

    #[test]
    fn deploy_prepare_rejects_empty_idempotency_key() {
        let mut request = sample_deploy_prepare_request();
        request.idempotency_key.clear();
        assert!(request.validate().is_err());
    }

    #[test]
    fn artifact_push_validates_range_and_hash_header() {
        let header = ArtifactPushChunkHeader {
            deploy_id: "dep-1".to_string(),
            offset: 0,
            length: 0,
            chunk_sha256: "".to_string(),
            upload_token: "token".to_string(),
        };
        assert!(header.validate().is_err());

        let ack = ArtifactPushAck {
            received_ranges: vec![ByteRange {
                offset: 0,
                length: 0,
            }],
            next_missing_range: None,
            accepted_bytes: 0,
        };
        assert!(ack.validate().is_err());
    }

    #[test]
    fn artifact_push_request_round_trip_and_validate() {
        let request = ArtifactPushRequest {
            header: ArtifactPushChunkHeader {
                deploy_id: "dep-1".to_string(),
                offset: 0,
                length: 4,
                chunk_sha256: "abcd".to_string(),
                upload_token: "token".to_string(),
            },
            chunk_b64: "AQIDBA==".to_string(),
        };

        request.validate().expect("request should be valid");
        let encoded = to_cbor(&request).expect("encoding should succeed");
        let decoded: ArtifactPushRequest = from_cbor(&encoded).expect("decoding should succeed");
        assert_eq!(decoded, request);
    }

    #[derive(Debug, Serialize)]
    struct ArtifactPushRequestMissingChunk<'a> {
        #[serde(flatten)]
        header: ArtifactPushChunkHeaderBorrowed<'a>,
    }

    #[derive(Debug, Serialize)]
    struct ArtifactPushChunkHeaderBorrowed<'a> {
        deploy_id: &'a str,
        offset: u64,
        length: u64,
        chunk_sha256: &'a str,
        upload_token: &'a str,
    }

    #[test]
    fn artifact_push_request_rejects_missing_chunk_b64() {
        let encoded = to_cbor(&ArtifactPushRequestMissingChunk {
            header: ArtifactPushChunkHeaderBorrowed {
                deploy_id: "dep-1",
                offset: 0,
                length: 4,
                chunk_sha256: "abcd",
                upload_token: "token",
            },
        })
        .expect("encoding should succeed");
        let decoded = from_cbor::<ArtifactPushRequest>(&encoded);
        assert!(decoded.is_err());
    }

    #[test]
    fn artifact_commit_rejects_missing_required_values() {
        let request = ArtifactCommitRequest {
            deploy_id: "dep-1".to_string(),
            artifact_digest: "".to_string(),
            artifact_size: 0,
            manifest_digest: "".to_string(),
        };

        assert!(request.validate().is_err());
    }

    #[test]
    fn command_start_validates_each_payload_type() {
        let deploy = CommandStartRequest {
            request_id: sample_request_id(),
            command_type: CommandType::Deploy,
            payload: CommandPayload::Deploy(DeployCommandPayload {
                deploy_id: "dep-1".to_string(),
                expected_current_release: "rel-1".to_string(),
                restart_policy: "never".to_string(),
                auto_rollback: true,
            }),
        };
        assert!(deploy.validate().is_ok());

        let run = CommandStartRequest {
            request_id: sample_request_id(),
            command_type: CommandType::Run,
            payload: CommandPayload::Run(RunCommandPayload {
                name: "syslog-forwarder".to_string(),
            }),
        };
        assert!(run.validate().is_ok());

        let stop = CommandStartRequest {
            request_id: sample_request_id(),
            command_type: CommandType::Stop,
            payload: CommandPayload::Stop(StopCommandPayload {
                name: "syslog-forwarder".to_string(),
                force: false,
            }),
        };
        assert!(stop.validate().is_ok());
    }

    #[test]
    fn command_start_rejects_payload_command_mismatch() {
        let request = CommandStartRequest {
            request_id: sample_request_id(),
            command_type: CommandType::Run,
            payload: CommandPayload::Deploy(DeployCommandPayload {
                deploy_id: "dep-1".to_string(),
                expected_current_release: "rel-1".to_string(),
                restart_policy: "never".to_string(),
                auto_rollback: true,
            }),
        };
        assert!(request.validate().is_err());
    }

    #[derive(Debug, Serialize)]
    struct DeployPayloadWithoutAutoRollback<'a> {
        deploy_id: &'a str,
        expected_current_release: &'a str,
        restart_policy: &'a str,
    }

    #[test]
    fn deploy_payload_defaults_auto_rollback_to_true() {
        let encoded = to_cbor(&DeployPayloadWithoutAutoRollback {
            deploy_id: "dep-1",
            expected_current_release: "rel-1",
            restart_policy: "never",
        })
        .expect("encoding should succeed");

        let decoded: DeployCommandPayload = from_cbor(&encoded).expect("decoding should succeed");
        assert!(decoded.auto_rollback);
    }

    #[test]
    fn command_event_enforces_progress_and_failed_requirements() {
        let progress = CommandEvent {
            event_type: CommandEventType::Progress,
            request_id: sample_request_id(),
            command_type: CommandType::Deploy,
            timestamp: "2026-02-10T00:00:00Z".to_string(),
            stage: None,
            error: None,
        };
        assert!(progress.validate().is_err());

        let failed = CommandEvent {
            event_type: CommandEventType::Failed,
            request_id: sample_request_id(),
            command_type: CommandType::Deploy,
            timestamp: "2026-02-10T00:00:01Z".to_string(),
            stage: Some("commit".to_string()),
            error: None,
        };
        assert!(failed.validate().is_err());
    }

    #[test]
    fn state_request_and_response_validate_required_fields() {
        let invalid_request = StateRequest {
            request_id: Uuid::nil(),
        };
        assert!(invalid_request.validate().is_err());

        let invalid_response = StateResponse {
            request_id: sample_request_id(),
            state: CommandState::Running,
            stage: "".to_string(),
            updated_at: "".to_string(),
        };
        assert!(invalid_response.validate().is_err());
    }

    #[test]
    fn state_response_rejects_terminal_states() {
        let succeeded = StateResponse {
            request_id: sample_request_id(),
            state: CommandState::Succeeded,
            stage: "done".to_string(),
            updated_at: "2026-02-10T00:00:00Z".to_string(),
        };
        assert!(succeeded.validate().is_err());

        let failed = StateResponse {
            request_id: sample_request_id(),
            state: CommandState::Failed,
            stage: "rollback".to_string(),
            updated_at: "2026-02-10T00:00:01Z".to_string(),
        };
        assert!(failed.validate().is_err());

        let canceled = StateResponse {
            request_id: sample_request_id(),
            state: CommandState::Canceled,
            stage: "cancel".to_string(),
            updated_at: "2026-02-10T00:00:02Z".to_string(),
        };
        assert!(canceled.validate().is_err());

        let running = StateResponse {
            request_id: sample_request_id(),
            state: CommandState::Running,
            stage: "deploying".to_string(),
            updated_at: "2026-02-10T00:00:03Z".to_string(),
        };
        assert!(running.validate().is_ok());
    }

    #[derive(Debug, Serialize)]
    struct CommandCancelMissingFinalState {
        cancellable: bool,
    }

    #[test]
    fn command_cancel_validates_request_and_response_shape() {
        let invalid_request = CommandCancelRequest {
            request_id: Uuid::nil(),
        };
        assert!(invalid_request.validate().is_err());

        let encoded = to_cbor(&CommandCancelMissingFinalState { cancellable: true })
            .expect("encoding should succeed");
        let decoded = from_cbor::<CommandCancelResponse>(&encoded);
        assert!(decoded.is_err());
    }

    #[test]
    fn log_request_accepts_optional_name() {
        let all = LogRequest {
            name: None,
            follow: false,
            tail_lines: 0,
        };
        assert!(all.validate().is_ok());

        let named = LogRequest {
            name: Some("svc-a".to_string()),
            follow: true,
            tail_lines: 200,
        };
        assert!(named.validate().is_ok());

        let encoded = to_cbor(&named).expect("encoding should succeed");
        let decoded = from_cbor::<LogRequest>(&encoded).expect("decoding should succeed");
        assert_eq!(decoded, named);
    }

    #[test]
    fn log_chunk_requires_request_id_and_name() {
        let invalid = LogChunk {
            request_id: Uuid::nil(),
            seq: 1,
            name: "".to_string(),
            stream_kind: LogStreamKind::Stdout,
            bytes: b"abc".to_vec(),
            is_last: false,
        };
        assert!(invalid.validate().is_err());

        let valid = LogChunk {
            request_id: sample_request_id(),
            seq: 2,
            name: "svc-a".to_string(),
            stream_kind: LogStreamKind::Composite,
            bytes: b"line\n".to_vec(),
            is_last: true,
        };
        assert!(valid.validate().is_ok());
    }

    #[test]
    fn log_end_round_trip_with_error() {
        let end = LogEnd {
            request_id: sample_request_id(),
            seq: 10,
            error: Some(LogError {
                code: LogErrorCode::ProcessNotRunning,
                message: "service is not running".to_string(),
            }),
        };

        end.validate().expect("log end should be valid");
        let encoded = to_cbor(&end).expect("encoding should succeed");
        let decoded = from_cbor::<LogEnd>(&encoded).expect("decoding should succeed");
        assert_eq!(decoded, end);
    }

    #[test]
    fn service_list_round_trip_and_validate() {
        let request = ServiceListRequest {
            names: Some(vec!["svc-a".to_string(), "svc-b".to_string()]),
        };
        request.validate().expect("request should be valid");
        let encoded = to_cbor(&request).expect("encoding should succeed");
        let decoded = from_cbor::<ServiceListRequest>(&encoded).expect("decoding should succeed");
        assert_eq!(decoded, request);

        let response = ServiceListResponse {
            services: vec![ServiceStatusEntry {
                name: "svc-a".to_string(),
                release_hash: "release-a".to_string(),
                started_at: "1735689600".to_string(),
                state: ServiceState::Running,
            }],
        };
        response.validate().expect("response should be valid");
        let encoded = to_cbor(&response).expect("encoding should succeed");
        let decoded = from_cbor::<ServiceListResponse>(&encoded).expect("decoding should succeed");
        assert_eq!(decoded, response);
    }

    #[test]
    fn service_list_request_rejects_empty_or_duplicate_names() {
        let empty_name = ServiceListRequest {
            names: Some(vec!["svc-a".to_string(), "".to_string()]),
        };
        assert!(empty_name.validate().is_err());

        let duplicate_name = ServiceListRequest {
            names: Some(vec!["svc-a".to_string(), "svc-a".to_string()]),
        };
        assert!(duplicate_name.validate().is_err());
    }

    #[test]
    fn service_list_response_allows_empty_started_at_when_stopped() {
        let response = ServiceListResponse {
            services: vec![ServiceStatusEntry {
                name: "svc-a".to_string(),
                release_hash: "release-a".to_string(),
                started_at: "".to_string(),
                state: ServiceState::Stopped,
            }],
        };
        assert!(response.validate().is_ok());
    }

    #[test]
    fn service_list_response_requires_started_at_for_running_and_stopping() {
        for state in [ServiceState::Running, ServiceState::Stopping] {
            let invalid = ServiceListResponse {
                services: vec![ServiceStatusEntry {
                    name: "svc-a".to_string(),
                    release_hash: "release-a".to_string(),
                    started_at: "".to_string(),
                    state,
                }],
            };
            assert!(
                invalid.validate().is_err(),
                "state {state:?} should require non-empty started_at"
            );
        }
    }

    #[test]
    fn service_list_response_rejects_empty_required_fields() {
        let invalid = ServiceListResponse {
            services: vec![ServiceStatusEntry {
                name: "svc-a".to_string(),
                release_hash: "".to_string(),
                started_at: "1735689600".to_string(),
                state: ServiceState::Stopped,
            }],
        };
        assert!(invalid.validate().is_err());
    }
}
