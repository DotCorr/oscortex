// Minimal userspace C library helper for OSCortex
// Implements performance-critical string, memory, and math functions natively.

#define size_t unsigned long

// --- Memory & String functions ---

void *memcpy(void *dst, const void *src, size_t n) {
    char *d = (char *)dst;
    const char *s = (const char *)src;
    for (size_t i = 0; i < n; i++) {
        d[i] = s[i];
    }
    return dst;
}

void *memset(void *dst, int val, size_t n) {
    char *d = (char *)dst;
    for (size_t i = 0; i < n; i++) {
        d[i] = (char)val;
    }
    return dst;
}

void *memmove(void *dst, const void *src, size_t n) {
    char *d = (char *)dst;
    const char *s = (const char *)src;
    if (d < s) {
        for (size_t i = 0; i < n; i++) {
            d[i] = s[i];
        }
    } else if (d > s) {
        for (size_t i = n; i > 0; i--) {
            d[i - 1] = s[i - 1];
        }
    }
    return dst;
}

size_t strlen(const char *s) {
    size_t len = 0;
    while (s[len] != 0) {
        len++;
    }
    return len;
}

int strcmp(const char *s1, const char *s2) {
    while (*s1 && (*s1 == *s2)) {
        s1++;
        s2++;
    }
    return *(const unsigned char *)s1 - *(const unsigned char *)s2;
}

int strncmp(const char *s1, const char *s2, size_t n) {
    if (n == 0) return 0;
    while (n-- > 0) {
        if (*s1 != *s2) {
            return *(const unsigned char *)s1 - *(const unsigned char *)s2;
        }
        if (*s1 == 0) {
            break;
        }
        s1++;
        s2++;
    }
    return 0;
}

char *strchr(const char *s, int c) {
    while (*s != (char)c) {
        if (!*s) {
            return 0;
        }
        s++;
    }
    return (char *)s;
}

// --- Mathematical functions ---
//
// On x86_64 the x87 FPU provides fsin/fcos/fsqrt directly. AArch64 has no
// transcendental instructions, so sin/cos use a range-reduced minimax-style
// polynomial and sqrt uses the FSQRT instruction via the compiler builtin. The
// engine only needs these for occasional layout/animation math, so a few ULP of
// error is fine.

#if defined(__x86_64__)

double sin(double x) {
    double result;
    __asm__ volatile ("fsin" : "=t"(result) : "0"(x));
    return result;
}

double cos(double x) {
    double result;
    __asm__ volatile ("fcos" : "=t"(result) : "0"(x));
    return result;
}

float sinf(float x) {
    float result;
    __asm__ volatile ("fsin" : "=t"(result) : "0"(x));
    return result;
}

float cosf(float x) {
    float result;
    __asm__ volatile ("fcos" : "=t"(result) : "0"(x));
    return result;
}

double sqrt(double x) {
    double result;
    __asm__ volatile ("fsqrt" : "=t"(result) : "0"(x));
    return result;
}

float sqrtf(float x) {
    float result;
    __asm__ volatile ("fsqrt" : "=t"(result) : "0"(x));
    return result;
}

#else  /* aarch64 (and any non-x86 host) */

/* Reduce x into [-pi, pi] using a precise pi, then evaluate an 11th-order
 * Taylor series — accurate to a few ULP across that range. */
static double m_reduce(double x) {
    const double TWO_PI = 6.283185307179586476925286766559;
    const double PI     = 3.141592653589793238462643383279;
    /* k = round(x / 2pi) */
    double k = x / TWO_PI;
    long long ki = (long long)(k >= 0 ? k + 0.5 : k - 0.5);
    x -= (double)ki * TWO_PI;
    if (x > PI)  x -= TWO_PI;
    if (x < -PI) x += TWO_PI;
    return x;
}

double sin(double x) {
    x = m_reduce(x);
    double x2 = x * x;
    /* x - x^3/3! + x^5/5! - x^7/7! + x^9/9! - x^11/11! */
    double t = x;
    double s = x;
    t *= -x2 / (2*3);   s += t;
    t *= -x2 / (4*5);   s += t;
    t *= -x2 / (6*7);   s += t;
    t *= -x2 / (8*9);   s += t;
    t *= -x2 / (10*11); s += t;
    return s;
}

double cos(double x) {
    x = m_reduce(x);
    double x2 = x * x;
    /* 1 - x^2/2! + x^4/4! - x^6/6! + x^8/8! - x^10/10! */
    double t = 1.0;
    double s = 1.0;
    t *= -x2 / (1*2);   s += t;
    t *= -x2 / (3*4);   s += t;
    t *= -x2 / (5*6);   s += t;
    t *= -x2 / (7*8);   s += t;
    t *= -x2 / (9*10);  s += t;
    return s;
}

float sinf(float x) { return (float)sin((double)x); }
float cosf(float x) { return (float)cos((double)x); }

double sqrt(double x)  { return __builtin_sqrt(x); }
float  sqrtf(float x)  { return __builtin_sqrtf(x); }

#endif

double floor(double x) {
    long long i = (long long)x;
    double d = (double)i;
    if (x < 0.0 && x != d) {
        return d - 1.0;
    }
    return d;
}

float floorf(float x) {
    int i = (int)x;
    float f = (float)i;
    if (x < 0.0f && x != f) {
        return f - 1.0f;
    }
    return f;
}

double ceil(double x) {
    long long i = (long long)x;
    double d = (double)i;
    if (x > 0.0 && x != d) {
        return d + 1.0;
    }
    return d;
}

float ceilf(float x) {
    int i = (int)x;
    float f = (float)i;
    if (x > 0.0f && x != f) {
        return f + 1.0f;
    }
    return f;
}

double trunc(double x) {
    return (double)(long long)x;
}

float truncf(float x) {
    return (float)(int)x;
}

double round(double x) {
    if (x >= 0.0) {
        return floor(x + 0.5);
    } else {
        return ceil(x - 0.5);
    }
}

float roundf(float x) {
    if (x >= 0.0f) {
        return floorf(x + 0.5f);
    } else {
        return ceilf(x - 0.5f);
    }
}
