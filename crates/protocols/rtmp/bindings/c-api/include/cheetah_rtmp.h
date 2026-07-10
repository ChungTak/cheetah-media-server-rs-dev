#ifndef CHEETAH_RTMP_H
#define CHEETAH_RTMP_H

/* Generated with cbindgen:0.29.2 */

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef enum RtmpCoreApiError {
  RTMP_CORE_API_ERROR_OK = 0,
  RTMP_CORE_API_ERROR_INVALID_ARGUMENT,
  RTMP_CORE_API_ERROR_NULL_POINTER,
  RTMP_CORE_API_ERROR_CORE,
  RTMP_CORE_API_ERROR_NO_OUTPUT,
  RTMP_CORE_API_ERROR_OVERFLOW,
} RtmpCoreApiError;

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

typedef enum RtmpCoreOutputMediaType {
  RTMP_CORE_OUTPUT_MEDIA_TYPE_NONE = 0,
  RTMP_CORE_OUTPUT_MEDIA_TYPE_AUDIO,
  RTMP_CORE_OUTPUT_MEDIA_TYPE_VIDEO,
  RTMP_CORE_OUTPUT_MEDIA_TYPE_DATA,
} RtmpCoreOutputMediaType;

typedef struct RtmpCoreHandle RtmpCoreHandle;

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

const char *rtmp_library_version(void);

struct RtmpCoreHandle *rtmp_core_new(void);

void rtmp_core_free(struct RtmpCoreHandle *handle);

const char *rtmp_core_get_last_error(const struct RtmpCoreHandle *handle);

uint32_t rtmp_core_pending_output_count(const struct RtmpCoreHandle *handle);

void rtmp_core_clear_outputs(struct RtmpCoreHandle *handle);

void rtmp_core_clear_output(struct RtmpCoreHandle *handle);

enum RtmpCoreApiError rtmp_core_next_output(struct RtmpCoreHandle *handle,
                                            struct RtmpCoreOutputView *output);

enum RtmpCoreApiError rtmp_core_handle_bytes(struct RtmpCoreHandle *handle,
                                             const uint8_t *data,
                                             uint32_t len);

enum RtmpCoreApiError rtmp_core_handle_timeout(struct RtmpCoreHandle *handle, uint64_t timer_id);

enum RtmpCoreApiError rtmp_core_command_accept_publish(struct RtmpCoreHandle *handle,
                                                       uint32_t stream_id);

enum RtmpCoreApiError rtmp_core_command_reject_publish(struct RtmpCoreHandle *handle,
                                                       uint32_t stream_id,
                                                       const uint8_t *description_ptr,
                                                       uint32_t description_len);

enum RtmpCoreApiError rtmp_core_command_accept_play(struct RtmpCoreHandle *handle,
                                                    uint32_t stream_id);

enum RtmpCoreApiError rtmp_core_command_accept_play_configured(struct RtmpCoreHandle *handle,
                                                               uint32_t stream_id,
                                                               bool emit_play_status,
                                                               bool emit_sample_access);

enum RtmpCoreApiError rtmp_core_command_reject_play(struct RtmpCoreHandle *handle,
                                                    uint32_t stream_id,
                                                    const uint8_t *description_ptr,
                                                    uint32_t description_len);

enum RtmpCoreApiError rtmp_core_command_send_metadata(struct RtmpCoreHandle *handle,
                                                      uint32_t stream_id,
                                                      uint32_t timestamp_ms,
                                                      const uint8_t *payload_ptr,
                                                      uint32_t payload_len);

enum RtmpCoreApiError rtmp_core_command_send_audio(struct RtmpCoreHandle *handle,
                                                   uint32_t stream_id,
                                                   uint32_t timestamp_ms,
                                                   const uint8_t *payload_ptr,
                                                   uint32_t payload_len);

enum RtmpCoreApiError rtmp_core_command_send_video(struct RtmpCoreHandle *handle,
                                                   uint32_t stream_id,
                                                   uint32_t timestamp_ms,
                                                   const uint8_t *payload_ptr,
                                                   uint32_t payload_len);

enum RtmpCoreApiError rtmp_core_command_send_notify(struct RtmpCoreHandle *handle,
                                                    uint32_t stream_id,
                                                    uint32_t timestamp_ms,
                                                    const uint8_t *payload_ptr,
                                                    uint32_t payload_len);

enum RtmpCoreApiError rtmp_core_command_close_stream(struct RtmpCoreHandle *handle,
                                                     uint32_t stream_id);

enum RtmpCoreApiError rtmp_core_command_close_connection(struct RtmpCoreHandle *handle);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* CHEETAH_RTMP_H */
