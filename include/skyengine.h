#ifndef SKYENGINE_H
#define SKYENGINE_H

#include <stdint.h>

#if defined(_WIN32)
#define SKYENGINE_EXPORT __declspec(dllimport)
#else
#define SKYENGINE_EXPORT __attribute__((visibility("default")))
#endif

#ifdef __cplusplus
extern "C" {
#endif

SKYENGINE_EXPORT int32_t skyengine_api_init(int32_t width, int32_t height);
SKYENGINE_EXPORT int32_t skyengine_api_set_memory(int32_t memory_mb);
SKYENGINE_EXPORT int32_t skyengine_api_set_device_date(const char *date);
SKYENGINE_EXPORT int32_t skyengine_api_set_work_dir(const char *path);
/* Sets an SF2 General MIDI bank before start. Relative paths use the work directory. */
SKYENGINE_EXPORT int32_t skyengine_api_set_sound_font(const char *path);
SKYENGINE_EXPORT int32_t skyengine_api_set_dns_map(const char *mappings);
SKYENGINE_EXPORT int32_t skyengine_api_start(
    const char *mrp_path,
    const char *entry,
    const char *entry_override);
SKYENGINE_EXPORT void skyengine_api_destroy(void);
SKYENGINE_EXPORT int32_t skyengine_api_is_running(void);
SKYENGINE_EXPORT int32_t skyengine_api_pause(void);
SKYENGINE_EXPORT int32_t skyengine_api_resume(void);
SKYENGINE_EXPORT int32_t skyengine_api_event(
    int32_t event,
    int32_t parameter0,
    int32_t parameter1);

SKYENGINE_EXPORT int32_t skyengine_api_timer(void);
SKYENGINE_EXPORT int32_t skyengine_api_get_timer_interval(void);
SKYENGINE_EXPORT int32_t skyengine_api_set_image_processing_mode(int32_t mode);
SKYENGINE_EXPORT int32_t skyengine_api_get_image_processing_mode(void);

SKYENGINE_EXPORT const uint16_t *skyengine_api_get_screen_buffer(void);
SKYENGINE_EXPORT const uint8_t *skyengine_api_get_screen_rgba_buffer(void);
SKYENGINE_EXPORT int32_t skyengine_api_get_screen_dirty(void);
SKYENGINE_EXPORT int32_t skyengine_api_get_screen_width(void);
SKYENGINE_EXPORT int32_t skyengine_api_get_screen_height(void);
SKYENGINE_EXPORT int32_t skyengine_api_get_screen_rotation(void);

SKYENGINE_EXPORT int32_t skyengine_api_audio_sample_rate(void);
SKYENGINE_EXPORT int32_t skyengine_api_audio_channels(void);
SKYENGINE_EXPORT int32_t skyengine_api_audio_is_active(void);
/* Writes interleaved stereo S16LE PCM and returns the number of audio frames written.
 * output must be suitably aligned for int16_t and hold frames * 2 samples. */
SKYENGINE_EXPORT int32_t skyengine_api_audio_render_s16le(
    void *output,
    int32_t frames);
SKYENGINE_EXPORT void skyengine_api_audio_stop(void);

SKYENGINE_EXPORT int32_t skyengine_api_is_edit_active(void);
SKYENGINE_EXPORT const char *skyengine_api_get_edit_text(void);
SKYENGINE_EXPORT int32_t skyengine_api_set_edit_text(const char *text);
SKYENGINE_EXPORT int32_t skyengine_api_cancel_edit(void);

SKYENGINE_EXPORT int32_t skyengine_api_motion(int32_t x, int32_t y, int32_t z);
SKYENGINE_EXPORT int32_t skyengine_api_motion_active(void);
SKYENGINE_EXPORT int32_t skyengine_api_take_shake(void);

/// Returns a UTF-8 description of the most recent bridge error.
/// The pointer remains valid until the next SkyEngine API call on this process.
SKYENGINE_EXPORT const char *skyengine_api_last_error(void);

#ifdef __cplusplus
}
#endif

#endif
