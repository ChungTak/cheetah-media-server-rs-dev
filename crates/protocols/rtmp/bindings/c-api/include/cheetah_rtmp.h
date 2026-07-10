#ifndef CHEETAH_RTMP_H
#define CHEETAH_RTMP_H

/* Generated with cbindgen:0.29.2 */

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

/**
 * C error codes returned by the RTMP core C API.
 *
 * RTMP core C API 返回的错误码。
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
 * C enum identifying the kind of output produced by the RTMP core.
 *
 * 标识 RTMP core 输出类型的 C 枚举。
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
 * C enum for the media type carried in an output event.
 *
 * 输出事件中携带的媒体类型的 C 枚举。
 */
typedef enum RtmpCoreOutputMediaType {
  RTMP_CORE_OUTPUT_MEDIA_TYPE_NONE = 0,
  RTMP_CORE_OUTPUT_MEDIA_TYPE_AUDIO,
  RTMP_CORE_OUTPUT_MEDIA_TYPE_VIDEO,
  RTMP_CORE_OUTPUT_MEDIA_TYPE_DATA,
} RtmpCoreOutputMediaType;

/**
 * Opaque handle that owns an `RtmpCore` and a queue of pending outputs.
 *
 * 拥有 `RtmpCore` 与待处理输出队列的不透明句柄。
 */
typedef struct RtmpCoreHandle RtmpCoreHandle;

/**
 * C-compatible view of a pending output; pointers reference owned Rust bytes.
 *
 * 待处理输出的 C 兼容视图；指针指向 Rust 拥有的字节。
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
 * Return the library version string as a static C string.
 *
 * 将库版本字符串作为静态 C 字符串返回。
 */
const char *rtmp_library_version(void);

/**
 * Allocate a new RTMP core handle.
 *
 * 分配新的 RTMP core 句柄。
 */
struct RtmpCoreHandle *rtmp_core_new(void);

/**
 * Free an RTMP core handle previously created by `rtmp_core_new`.
 *
 * 释放之前由 `rtmp_core_new` 创建的 RTMP core 句柄。
 */
void rtmp_core_free(struct RtmpCoreHandle *handle);

/**
 * Return the last error message for the handle, or an empty string if none.
 *
 * 返回句柄的上次错误消息，若无则返回空字符串。
 */
const char *rtmp_core_get_last_error(const struct RtmpCoreHandle *handle);

/**
 * Return the number of outputs queued on the handle.
 *
 * 返回句柄上已排队的输出数量。
 */
uint32_t rtmp_core_pending_output_count(const struct RtmpCoreHandle *handle);

/**
 * Drop all pending outputs from the handle.
 *
 * 丢弃句柄上所有待处理输出。
 */
void rtmp_core_clear_outputs(struct RtmpCoreHandle *handle);

/**
 * Drop the next pending output from the handle.
 *
 * 丢弃句柄上的下一个待处理输出。
 */
void rtmp_core_clear_output(struct RtmpCoreHandle *handle);

/**
 * Pop the next pending output and return a C view into its buffers.
 *
 * 弹出下一个待处理输出并返回其缓冲区的 C 视图。
 */
enum RtmpCoreApiError rtmp_core_next_output(struct RtmpCoreHandle *handle,
                                            struct RtmpCoreOutputView *output);

/**
 * Feed raw bytes into the core and enqueue any outputs.
 *
 * 将原始字节喂入 core 并入队任何输出。
 */
enum RtmpCoreApiError rtmp_core_handle_bytes(struct RtmpCoreHandle *handle,
                                             const uint8_t *data,
                                             uint32_t len);

/**
 * Notify the core that a timer has expired.
 *
 * 通知 core 定时器已到期。
 */
enum RtmpCoreApiError rtmp_core_handle_timeout(struct RtmpCoreHandle *handle, uint64_t timer_id);

/**
 * Accept a publish request for the given stream.
 *
 * 接受指定流的发布请求。
 */
enum RtmpCoreApiError rtmp_core_command_accept_publish(struct RtmpCoreHandle *handle,
                                                       uint32_t stream_id);

/**
 * Reject a publish request for the given stream with a description.
 *
 * 以描述信息拒绝指定流的发布请求。
 */
enum RtmpCoreApiError rtmp_core_command_reject_publish(struct RtmpCoreHandle *handle,
                                                       uint32_t stream_id,
                                                       const uint8_t *description_ptr,
                                                       uint32_t description_len);

/**
 * Accept a play request for the given stream.
 *
 * 接受指定流的播放请求。
 */
enum RtmpCoreApiError rtmp_core_command_accept_play(struct RtmpCoreHandle *handle,
                                                    uint32_t stream_id);

/**
 * Accept a play request with optional status/sample-access messages.
 *
 * 接受播放请求，并可选择发送状态与 sample-access 消息。
 */
enum RtmpCoreApiError rtmp_core_command_accept_play_configured(struct RtmpCoreHandle *handle,
                                                               uint32_t stream_id,
                                                               bool emit_play_status,
                                                               bool emit_sample_access);

/**
 * Reject a play request for the given stream with a description.
 *
 * 以描述信息拒绝指定流的播放请求。
 */
enum RtmpCoreApiError rtmp_core_command_reject_play(struct RtmpCoreHandle *handle,
                                                    uint32_t stream_id,
                                                    const uint8_t *description_ptr,
                                                    uint32_t description_len);

/**
 * Send metadata to the given stream.
 *
 * 向指定流发送元数据。
 */
enum RtmpCoreApiError rtmp_core_command_send_metadata(struct RtmpCoreHandle *handle,
                                                      uint32_t stream_id,
                                                      uint32_t timestamp_ms,
                                                      const uint8_t *payload_ptr,
                                                      uint32_t payload_len);

/**
 * Send audio data to the given stream.
 *
 * 向指定流发送音频数据。
 */
enum RtmpCoreApiError rtmp_core_command_send_audio(struct RtmpCoreHandle *handle,
                                                   uint32_t stream_id,
                                                   uint32_t timestamp_ms,
                                                   const uint8_t *payload_ptr,
                                                   uint32_t payload_len);

/**
 * Send video data to the given stream.
 *
 * 向指定流发送视频数据。
 */
enum RtmpCoreApiError rtmp_core_command_send_video(struct RtmpCoreHandle *handle,
                                                   uint32_t stream_id,
                                                   uint32_t timestamp_ms,
                                                   const uint8_t *payload_ptr,
                                                   uint32_t payload_len);

/**
 * Send a notify message to the given stream.
 *
 * 向指定流发送通知消息。
 */
enum RtmpCoreApiError rtmp_core_command_send_notify(struct RtmpCoreHandle *handle,
                                                    uint32_t stream_id,
                                                    uint32_t timestamp_ms,
                                                    const uint8_t *payload_ptr,
                                                    uint32_t payload_len);

/**
 * Close a stream by ID.
 *
 * 按 ID 关闭流。
 */
enum RtmpCoreApiError rtmp_core_command_close_stream(struct RtmpCoreHandle *handle,
                                                     uint32_t stream_id);

/**
 * Close the connection entirely.
 *
 * 完全关闭连接。
 */
enum RtmpCoreApiError rtmp_core_command_close_connection(struct RtmpCoreHandle *handle);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* CHEETAH_RTMP_H */
