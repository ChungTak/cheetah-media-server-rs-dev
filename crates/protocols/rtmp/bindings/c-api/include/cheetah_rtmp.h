#ifndef CHEETAH_RTMP_H
#define CHEETAH_RTMP_H

/* Generated with cbindgen:0.29.2 */

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

/**
 * Error returned by `RTMP Core API` operations.
 * `RTMP Core API` 操作返回的错误。
 */
typedef enum RtmpCoreApiError {
  RTMP_CORE_API_ERROR_OK = 0,
  RTMP_CORE_API_ERROR_INVALID_ARGUMENT,
  RTMP_CORE_API_ERROR_NULL_POINTER,
  RTMP_CORE_API_ERROR_CORE,
  RTMP_CORE_API_ERROR_NO_OUTPUT,
  RTMP_CORE_API_ERROR_OVERFLOW,
} RtmpCoreApiError;

/**
 * Kind of `RTMP Core Output`.
 * `RTMP Core Output` 的种类。
 */
typedef enum RtmpCoreOutputKind {
  RTMP_CORE_OUTPUT_KIND_NONE = 0,
  RTMP_CORE_OUTPUT_KIND_WRITE,
  RTMP_CORE_OUTPUT_KIND_EVENT_CONNECTED,
  RTMP_CORE_OUTPUT_KIND_EVENT_STREAM_CREATED,
  RTMP_CORE_OUTPUT_KIND_EVENT_COMMAND_IGNORED,
  RTMP_CORE_OUTPUT_KIND_EVENT_MESSAGE_IGNORED,
  RTMP_CORE_OUTPUT_KIND_EVENT_USER_CONTROL_IGNORED,
  RTMP_CORE_OUTPUT_KIND_EVENT_ACK_RECEIVED,
  RTMP_CORE_OUTPUT_KIND_EVENT_LOCAL_ACK_WINDOW_UPDATED,
  RTMP_CORE_OUTPUT_KIND_EVENT_PEER_ACK_WINDOW_UPDATED,
  RTMP_CORE_OUTPUT_KIND_EVENT_CLIENT_STATE_CHANGED,
  RTMP_CORE_OUTPUT_KIND_EVENT_CLIENT_DISCONNECT_REQUESTED,
  RTMP_CORE_OUTPUT_KIND_EVENT_PUBLISH_REQUESTED,
  RTMP_CORE_OUTPUT_KIND_EVENT_PLAY_REQUESTED,
  RTMP_CORE_OUTPUT_KIND_EVENT_METADATA,
  RTMP_CORE_OUTPUT_KIND_EVENT_NOTIFY,
  RTMP_CORE_OUTPUT_KIND_EVENT_MEDIA_DATA,
  RTMP_CORE_OUTPUT_KIND_EVENT_STREAM_CLOSED,
  RTMP_CORE_OUTPUT_KIND_EVENT_PEER_CLOSED,
  RTMP_CORE_OUTPUT_KIND_SET_TIMER,
  RTMP_CORE_OUTPUT_KIND_CANCEL_TIMER,
} RtmpCoreOutputKind;

/**
 * Type of `RTMP Core Output Media`.
 * `RTMP Core Output Media` 的类型。
 */
typedef enum RtmpCoreOutputMediaType {
  RTMP_CORE_OUTPUT_MEDIA_TYPE_NONE = 0,
  RTMP_CORE_OUTPUT_MEDIA_TYPE_AUDIO,
  RTMP_CORE_OUTPUT_MEDIA_TYPE_VIDEO,
  RTMP_CORE_OUTPUT_MEDIA_TYPE_DATA,
} RtmpCoreOutputMediaType;

/**
 * Handle to a `RTMP Core` resource.
 * `RTMP Core` 资源的句柄。
 */
typedef struct RtmpCoreHandle RtmpCoreHandle;

/**
 * View of `RTMP Core Output`.
 * `RTMP Core Output` 的视图。
 */
typedef struct RtmpCoreOutputView {
  enum RtmpCoreOutputKind kind;
  uint64_t timer_id;
  uint64_t at_micros;
  uint32_t stream_id;
  uint32_t timestamp_ms;
  enum RtmpCoreOutputMediaType media_type;
  const uint8_t *primary_ptr;
  uint32_t primary_len;
  const uint8_t *secondary_ptr;
  uint32_t secondary_len;
} RtmpCoreOutputView;

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

/**
 * `rtmp_library_version` function.
 * `rtmp_library_version` 函数。
 */
const char *rtmp_library_version(void);

/**
 * `rtmp_core_new` function.
 * `rtmp_core_new` 函数。
 */
struct RtmpCoreHandle *rtmp_core_new(void);

/**
 * `rtmp_core_free` function.
 * `rtmp_core_free` 函数。
 */
void rtmp_core_free(struct RtmpCoreHandle *handle);

/**
 * `rtmp_core_get_last_error` function.
 * `rtmp_core_get_last_error` 函数。
 */
const char *rtmp_core_get_last_error(const struct RtmpCoreHandle *handle);

/**
 * `rtmp_core_pending_output_count` function.
 * `rtmp_core_pending_output_count` 函数。
 */
uint32_t rtmp_core_pending_output_count(const struct RtmpCoreHandle *handle);

/**
 * `rtmp_core_clear_outputs` function.
 * `rtmp_core_clear_outputs` 函数。
 */
void rtmp_core_clear_outputs(struct RtmpCoreHandle *handle);

/**
 * `rtmp_core_clear_output` function.
 * `rtmp_core_clear_output` 函数。
 */
void rtmp_core_clear_output(struct RtmpCoreHandle *handle);

/**
 * `rtmp_core_next_output` function.
 * `rtmp_core_next_output` 函数。
 */
enum RtmpCoreApiError rtmp_core_next_output(struct RtmpCoreHandle *handle,
                                            struct RtmpCoreOutputView *output);

/**
 * `rtmp_core_handle_bytes` function.
 * `rtmp_core_handle_bytes` 函数。
 */
enum RtmpCoreApiError rtmp_core_handle_bytes(struct RtmpCoreHandle *handle,
                                             const uint8_t *data,
                                             uint32_t len);

/**
 * `rtmp_core_handle_timeout` function.
 * `rtmp_core_handle_timeout` 函数。
 */
enum RtmpCoreApiError rtmp_core_handle_timeout(struct RtmpCoreHandle *handle, uint64_t timer_id);

/**
 * `rtmp_core_command_accept_publish` function.
 * `rtmp_core_command_accept_publish` 函数。
 */
enum RtmpCoreApiError rtmp_core_command_accept_publish(struct RtmpCoreHandle *handle,
                                                       uint32_t stream_id);

/**
 * `rtmp_core_command_reject_publish` function.
 * `rtmp_core_command_reject_publish` 函数。
 */
enum RtmpCoreApiError rtmp_core_command_reject_publish(struct RtmpCoreHandle *handle,
                                                       uint32_t stream_id,
                                                       const uint8_t *description_ptr,
                                                       uint32_t description_len);

/**
 * `rtmp_core_command_accept_play` function.
 * `rtmp_core_command_accept_play` 函数。
 */
enum RtmpCoreApiError rtmp_core_command_accept_play(struct RtmpCoreHandle *handle,
                                                    uint32_t stream_id);

/**
 * `rtmp_core_command_accept_play_configured` function.
 * `rtmp_core_command_accept_play_configured` 函数。
 */
enum RtmpCoreApiError rtmp_core_command_accept_play_configured(struct RtmpCoreHandle *handle,
                                                               uint32_t stream_id,
                                                               bool emit_play_status,
                                                               bool emit_sample_access);

/**
 * `rtmp_core_command_reject_play` function.
 * `rtmp_core_command_reject_play` 函数。
 */
enum RtmpCoreApiError rtmp_core_command_reject_play(struct RtmpCoreHandle *handle,
                                                    uint32_t stream_id,
                                                    const uint8_t *description_ptr,
                                                    uint32_t description_len);

/**
 * `rtmp_core_command_send_metadata` function.
 * `rtmp_core_command_send_metadata` 函数。
 */
enum RtmpCoreApiError rtmp_core_command_send_metadata(struct RtmpCoreHandle *handle,
                                                      uint32_t stream_id,
                                                      uint32_t timestamp_ms,
                                                      const uint8_t *payload_ptr,
                                                      uint32_t payload_len);

/**
 * `rtmp_core_command_send_audio` function.
 * `rtmp_core_command_send_audio` 函数。
 */
enum RtmpCoreApiError rtmp_core_command_send_audio(struct RtmpCoreHandle *handle,
                                                   uint32_t stream_id,
                                                   uint32_t timestamp_ms,
                                                   const uint8_t *payload_ptr,
                                                   uint32_t payload_len);

/**
 * `rtmp_core_command_send_video` function.
 * `rtmp_core_command_send_video` 函数。
 */
enum RtmpCoreApiError rtmp_core_command_send_video(struct RtmpCoreHandle *handle,
                                                   uint32_t stream_id,
                                                   uint32_t timestamp_ms,
                                                   const uint8_t *payload_ptr,
                                                   uint32_t payload_len);

/**
 * `rtmp_core_command_send_notify` function.
 * `rtmp_core_command_send_notify` 函数。
 */
enum RtmpCoreApiError rtmp_core_command_send_notify(struct RtmpCoreHandle *handle,
                                                    uint32_t stream_id,
                                                    uint32_t timestamp_ms,
                                                    const uint8_t *payload_ptr,
                                                    uint32_t payload_len);

/**
 * `rtmp_core_command_close_stream` function.
 * `rtmp_core_command_close_stream` 函数。
 */
enum RtmpCoreApiError rtmp_core_command_close_stream(struct RtmpCoreHandle *handle,
                                                     uint32_t stream_id);

/**
 * `rtmp_core_command_close_connection` function.
 * `rtmp_core_command_close_connection` 函数。
 */
enum RtmpCoreApiError rtmp_core_command_close_connection(struct RtmpCoreHandle *handle);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* CHEETAH_RTMP_H */
