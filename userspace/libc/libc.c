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
