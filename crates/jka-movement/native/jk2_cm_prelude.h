/* Unmodified jk2mv collision sources (vendor/jk2mv) are compiled inside
 * namespace jk2mv_cm so their CM_* functions and globals cannot collide with
 * OpenJK's identically named collision. Every system header jk2mv's headers
 * include is included here first, outside the namespace; their include guards
 * then keep them out of it. */
#pragma once
#include <assert.h>
#include <ctype.h>
#include <errno.h>
#include <limits.h>
#include <math.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#include <cstddef>
#if defined(_M_X64) || defined(_M_IX86) || defined(__x86_64__) || defined(__i386__)
#include <emmintrin.h>
#endif
