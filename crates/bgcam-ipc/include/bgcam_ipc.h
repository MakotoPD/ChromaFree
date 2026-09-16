#pragma once

#include <stdint.h>

#define BGCAM_SECTION_NAME L"bgcam-frames-v1"

#define BGCAM_FRAME_READY_EVENT_NAME L"bgcam-frame-ready-v1"

#define BGCAM_CONSUMER_CHANGED_EVENT_NAME L"bgcam-consumer-changed-v1"


#define BGCAM_MAGIC 1296122690

#define BGCAM_PROTOCOL_VERSION 1

#define BGCAM_HEADER_SIZE 128

#define BGCAM_MAX_WIDTH 3840

#define BGCAM_MAX_HEIGHT 2160

#define BGCAM_FRAME_CAPACITY ((BGCAM_MAX_WIDTH * BGCAM_MAX_HEIGHT) * 4)

#define BGCAM_SECTION_SIZE (BGCAM_HEADER_SIZE + BGCAM_FRAME_CAPACITY)

#define BGCAM_FORMAT_NONE 0

#define BGCAM_FORMAT_NV12 1

#define BGCAM_FORMAT_BGRA 2

#define BGCAM_PRODUCER_ACTIVE 1

#define BGCAM_CONSUMER_ACTIVE 1

#define BGCAM_HEARTBEAT_TIMEOUT_MS 2000

#define BGCAM_SEQLOCK_ATTEMPTS 64

typedef struct {
  uint32_t magic;
  uint32_t version;
  uint32_t header_size;
  uint32_t capacity;
  uint32_t output_width;
  uint32_t output_height;
  uint32_t output_fps_numerator;
  uint32_t output_fps_denominator;
  uint32_t frame_width;
  uint32_t frame_height;
  uint32_t frame_format;
  uint32_t frame_size;
  uint32_t producer_flags;
  uint32_t consumer_flags;
  uint32_t consumer_format;
  uint32_t consumer_count;
  uint64_t sequence;
  uint64_t frame_number;
  int64_t frame_qpc;
  int64_t producer_heartbeat_qpc;
  int64_t consumer_heartbeat_qpc;
  int64_t qpc_frequency;
  uint32_t reserved[4];
} BgcamFrameHeader;
