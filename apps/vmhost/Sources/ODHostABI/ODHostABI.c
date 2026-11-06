/* The target ODHostABI is the header include/VMHostABI.h (engine C ABI declarations and VMHost helpers). SwiftPM builds a C
 * target only from at least one source file, so this unit compiles the header as plain C: it keeps the header
 * self-contained. It declares no symbol and does not link libod_host.a (only the `od-vmhost` target links it). */
#include "VMHostABI.h"
