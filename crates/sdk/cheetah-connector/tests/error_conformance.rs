use std::error::Error;
use std::io;

use cheetah_connector::{CloseReason, ConnectorError, Direction, Operation, Protocol};

#[test]
fn unsupported_protocol_is_not_retryable() {
    let err = ConnectorError::UnsupportedProtocol {
        protocol: Protocol::Rtmp,
        direction: Direction::Pull,
    };
    assert!(!err.retryable());
    assert_eq!(err.protocol(), Some(Protocol::Rtmp));
}

#[test]
fn backpressure_is_retryable() {
    let err = ConnectorError::Backpressure {
        protocol: Protocol::Rtmp,
    };
    assert!(err.retryable());
    assert_eq!(err.protocol(), Some(Protocol::Rtmp));
}

#[test]
fn closed_reasons_map_retryable_correctly() {
    let user = ConnectorError::Closed {
        protocol: Protocol::HttpFlv,
        reason: CloseReason::User,
    };
    assert!(!user.retryable());

    let error = ConnectorError::Closed {
        protocol: Protocol::HttpFlv,
        reason: CloseReason::Error("fatal".to_string()),
    };
    assert!(error.retryable());
}

#[test]
fn protocol_with_io_source_respects_retryable_kinds() {
    let transient = ConnectorError::Protocol {
        protocol: Protocol::HttpFlv,
        operation: Operation::Read,
        source: Box::new(io::Error::new(io::ErrorKind::ConnectionRefused, "refused")),
    };
    assert!(transient.retryable());

    let fatal = ConnectorError::Protocol {
        protocol: Protocol::HttpFlv,
        operation: Operation::Read,
        source: Box::new(io::Error::new(io::ErrorKind::PermissionDenied, "denied")),
    };
    assert!(!fatal.retryable());
}

#[test]
fn from_sdk_error_invalid_argument_not_retryable() {
    let err: ConnectorError = cheetah_sdk::SdkError::InvalidArgument("bad".to_string()).into();
    assert!(!err.retryable());
}

#[test]
fn from_sdk_error_unavailable_is_retryable_connect() {
    let err: ConnectorError = cheetah_sdk::SdkError::Unavailable("down".to_string()).into();
    assert!(err.retryable());
    assert!(matches!(err, ConnectorError::Connect { .. }));
    assert!(err.source().is_some());
}

#[cfg(feature = "http-flv")]
mod http_flv {
    use super::*;
    use cheetah_codec::FlvStreamError;
    use cheetah_http_flv_module::pull::HttpFlvPullError as E;

    #[test]
    fn invalid_url_and_scheme_are_not_retryable() {
        for err in [
            E::InvalidUrl("http://bad".to_string()),
            E::UnsupportedScheme {
                scheme: "ftp".to_string(),
            },
        ] {
            let mapped: ConnectorError = err.into();
            assert!(matches!(
                mapped,
                ConnectorError::InvalidUrl {
                    protocol: Protocol::HttpFlv,
                    ..
                }
            ));
            assert!(!mapped.retryable());
        }
    }

    #[test]
    fn connect_and_resolve_are_retryable() {
        for err in [
            E::Resolve("host".to_string()),
            E::Connect("host:80".to_string()),
        ] {
            let mapped: ConnectorError = err.into();
            assert!(matches!(
                mapped,
                ConnectorError::Connect {
                    protocol: Protocol::HttpFlv,
                    ..
                }
            ));
            assert!(mapped.retryable());
            assert!(mapped.source().is_some());
        }
    }

    #[test]
    fn cancelled_maps_to_closed() {
        let mapped: ConnectorError = E::Cancelled.into();
        assert!(matches!(
            mapped,
            ConnectorError::Closed {
                protocol: Protocol::HttpFlv,
                reason: CloseReason::Cancelled,
            }
        ));
        assert!(!mapped.retryable());
    }

    #[test]
    fn media_errors_not_retryable() {
        for err in [
            E::FlvDemux(FlvStreamError::InvalidHeaderSignature),
            E::Ingress("bad tag".to_string()),
        ] {
            let mapped: ConnectorError = err.into();
            assert!(matches!(mapped, ConnectorError::Media { codec: None, .. }));
            assert!(!mapped.retryable());
        }
    }

    #[test]
    fn read_and_write_are_retryable() {
        let read: ConnectorError = E::ReadBody("eof".to_string()).into();
        assert!(matches!(
            read,
            ConnectorError::Protocol {
                protocol: Protocol::HttpFlv,
                operation: Operation::Read,
                ..
            }
        ));
        assert!(read.retryable());

        let write: ConnectorError = E::WriteRequest("reset".to_string()).into();
        assert!(matches!(
            write,
            ConnectorError::Protocol {
                protocol: Protocol::HttpFlv,
                operation: Operation::Write,
                ..
            }
        ));
        assert!(write.retryable());
    }

    #[test]
    fn bad_status_code_retryable_only_for_5xx() {
        let server_err: ConnectorError = E::BadStatusCode { status_code: 503 }.into();
        assert!(matches!(
            server_err,
            ConnectorError::Protocol {
                protocol: Protocol::HttpFlv,
                operation: Operation::Play,
                ..
            }
        ));
        assert!(server_err.retryable());

        let client_err: ConnectorError = E::BadStatusCode { status_code: 404 }.into();
        assert!(matches!(
            client_err,
            ConnectorError::Protocol {
                protocol: Protocol::HttpFlv,
                operation: Operation::Play,
                ..
            }
        ));
        assert!(!client_err.retryable());
    }

    #[test]
    fn handshake_errors_not_retryable() {
        let err = E::ResponseHeaderIncomplete;
        let mapped: ConnectorError = err.into();
        assert!(matches!(
            mapped,
            ConnectorError::Protocol {
                protocol: Protocol::HttpFlv,
                operation: Operation::Handshake,
                ..
            }
        ));
        assert!(!mapped.retryable());
    }

    #[test]
    fn chunked_encoding_is_not_retryable() {
        let mapped: ConnectorError = E::InvalidChunkedEncoding("bad chunk".to_string()).into();
        assert!(matches!(
            mapped,
            ConnectorError::Protocol {
                protocol: Protocol::HttpFlv,
                operation: Operation::Read,
                ..
            }
        ));
        assert!(!mapped.retryable());
    }
}
