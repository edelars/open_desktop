/* The target ODHostConstants is the header include/ODHostConstants.h (engine host ABI constants). SwiftPM builds a C target
 * only from at least one source file, so this unit compiles the header as plain C. It declares no symbol of its own and
 * does not link libod_host.a (only the `od-vmhost` target links it). */
#include "ODHostConstants.h"
