#pragma once

#include <stdint.h>

#define CHROMAFREE_SECTION_NAME L"chromafree-frames-v1"

#define CHROMAFREE_FRAME_READY_EVENT_NAME L"chromafree-frame-ready-v1"

#define CHROMAFREE_CONSUMER_CHANGED_EVENT_NAME L"chromafree-consumer-changed-v1"


#define CHROMAFREE_MAGIC 1296122690

#define CHROMAFREE_PROTOCOL_VERSION 2

#define CHROMAFREE_HEADER_SIZE 128

#define CHROMAFREE_MAX_WIDTH 3840

#define CHROMAFREE_MAX_HEIGHT 2160

#define CHROMAFREE_FRAME_CAPACITY ((CHROMAFREE_MAX_WIDTH * CHROMAFREE_MAX_HEIGHT) * 4)

#define CHROMAFREE_SECTION_SIZE (CHROMAFREE_HEADER_SIZE + CHROMAFREE_FRAME_CAPACITY)

#define CHROMAFREE_FORMAT_NONE 0

#define CHROMAFREE_FORMAT_NV12 1

#define CHROMAFREE_FORMAT_BGRA 2

#define CHROMAFREE_PRODUCER_ACTIVE 1

#define CHROMAFREE_CONSUMER_ACTIVE 1

#define CHROMAFREE_HEARTBEAT_TIMEOUT_MS 2000

#define CHROMAFREE_SEQLOCK_ATTEMPTS 64

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
  uint32_t consumer_width;
  uint32_t consumer_height;
  uint32_t reserved[2];
} ChromaFreeFrameHeader;
