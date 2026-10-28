/* limits runtime: embedded verbatim into every generated C file. */
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <errno.h>
#include <sys/resource.h>

typedef struct { int64_t len; const char *d; } LmStr;
typedef LmStr *lm_str;
typedef union { int64_t i; double f; void *p; } lm_val;
typedef struct { int64_t len, cap; lm_val *d; } lm_list;

/* Large programs are split into several C files compiled in parallel
   (LM_SPLIT); then program functions and runtime globals need external linkage. */
#ifdef LM_SPLIT
#define LM_FN
#define LM_G extern
#define LM_DEF
#else
#define LM_FN static
#define LM_G static
#define LM_DEF static
#endif
LM_G int64_t lm_steps, lm_depth, lm_mem;
LM_G char *lm_ap, *lm_aend, *lm_stack_lo;

/* Source locations: line | file index << 20 (file 0 is the root file). */
#define LM_FILE_SHIFT 20
static const char *lm_file_name(int i); /* generated per program */
#define LM_COLD __attribute__((noreturn, cold, noinline))
LM_COLD static void lm_panic(int l, int c, const char *m) {
  fflush(stdout);
  int f = l >> LM_FILE_SHIFT;
  if (f) fprintf(stderr, "runtime error in %s at %d:%d: %s\n", lm_file_name(f), l & ((1 << LM_FILE_SHIFT) - 1), c, m);
  else fprintf(stderr, "runtime error at %d:%d: %s\n", l, c, m);
  exit(1);
}
/* The C stack is checked directly (not only the frame count), so neither large
   frames nor deep runtime recursion (printing) can overflow it. */
static void lm_stack_init(void) {
  struct rlimit r;
  size_t n = 8u << 20;
  if (getrlimit(RLIMIT_STACK, &r) == 0 && r.rlim_cur != RLIM_INFINITY) n = (size_t)r.rlim_cur;
  if (n > (1u << 30)) n = 1u << 30;
  n = n > (1u << 20) ? n - (512u << 10) : n / 2;
  lm_stack_lo = (char *)__builtin_frame_address(0) - n;
}
#define LM_STACK_OK() ((char *)__builtin_frame_address(0) >= lm_stack_lo)

LM_COLD static int lm_assert_fail(int l, int c, lm_str m) {
  if (!m) lm_panic(l, c, "assertion failed");
  char *b = malloc((size_t)m->len + 32);
  snprintf(b, (size_t)m->len + 32, "assertion failed: %s", m->d);
  lm_panic(l, c, b);
}

#ifdef LM_CHECKED
/* Steps are counted in a function-local copy (lm_ls) of the global budget so
   the hot loop counter lives in a register; it is written back before every
   call and on return, and reloaded after each call. Accounting stays exact. */
#define LM_STEP(l, c) if (__builtin_expect(--lm_ls < 0, 0)) lm_panic(l, c, "step limit exceeded")
#define LM_ENTER(l, c) int64_t lm_ls = lm_steps; LM_STEP(l, c); if (__builtin_expect(++lm_depth > LM_MAX_DEPTH || !LM_STACK_OK(), 0)) lm_panic(l, c, "call depth limit exceeded")
#define LM_LEAVE (lm_steps = lm_ls, lm_depth--)
#define LM_CALL(T, call) ({ lm_steps = lm_ls; T lm_r_ = call; lm_ls = lm_steps; lm_r_; })
#define LM_CALLV(call) ({ lm_steps = lm_ls; call; lm_ls = lm_steps; })
/* Builtins doing O(n) work charge one step per 64 bytes (caller wraps them in LM_CALL). */
static inline void lm_charge(int64_t n, int l, int c) {
  if (__builtin_expect((lm_steps -= n >> 6) < 0, 0)) lm_panic(l, c, "step limit exceeded");
}
#define LM_ARITH(name, op, bi) \
  static inline int64_t name(int64_t a, int64_t b, int l, int c) { \
    int64_t r; if (__builtin_expect(bi(a, b, &r), 0)) lm_panic(l, c, "integer overflow in '" op "'"); return r; }
#else
#define LM_STEP(l, c) ((void)0)
#define LM_ENTER(l, c) ((void)0)
#define LM_LEAVE ((void)0)
#define LM_CALL(T, call) (call)
#define LM_CALLV(call) (call)
#define lm_charge(n, l, c) ((void)0)
#define LM_ARITH(name, op, bi) \
  static inline int64_t name(int64_t a, int64_t b, int l, int c) { \
    (void)l; (void)c; int64_t r; bi(a, b, &r); return r; }
#endif
LM_ARITH(lm_add, "+", __builtin_add_overflow)
LM_ARITH(lm_sub, "-", __builtin_sub_overflow)
LM_ARITH(lm_mul, "*", __builtin_mul_overflow)

static inline int64_t lm_neg(int64_t a, int l, int c) { return lm_sub(0, a, l, c); }
static inline int64_t lm_div(int64_t a, int64_t b, int l, int c) {
  if (__builtin_expect(b == 0, 0)) lm_panic(l, c, "division by zero");
#ifdef LM_CHECKED
  if (__builtin_expect(b == -1 && a == INT64_MIN, 0)) lm_panic(l, c, "integer overflow in '/'");
#endif
  if (b == -1) return (int64_t)(0 - (uint64_t)a);
  return a / b;
}
static inline int64_t lm_mod(int64_t a, int64_t b, int l, int c) {
  if (__builtin_expect(b == 0, 0)) lm_panic(l, c, "division by zero");
  if (b == -1) return 0;
  return a % b;
}

/* Bump arena: memory is never freed (v0.1 limitation). Every heap byte is
   charged against the --max-mem budget (all modes). */
static void lm_mem_charge(size_t n, int l, int c) {
  if (n > (size_t)INT64_MAX || (lm_mem += (int64_t)n) > LM_MAX_MEM) lm_panic(l, c, "memory limit exceeded");
}
static void *lm_xmalloc(size_t n, int l, int c) {
  lm_mem_charge(n, l, c);
  void *p = malloc(n);
  if (!p) lm_panic(l, c, "out of memory");
  return p;
}
static void *lm_alloc(size_t n, int l, int c) {
  n = (n + 15) & ~(size_t)15;
  if (n > (size_t)(lm_aend - lm_ap)) {
    if (n > 65536) return lm_xmalloc(n, l, c);
    lm_ap = lm_xmalloc(1 << 20, l, c);
    lm_aend = lm_ap + (1 << 20);
  }
  void *p = lm_ap; lm_ap += n; return p;
}

static inline lm_val lm_vi(int64_t i) { lm_val v; v.i = i; return v; }
static inline lm_val lm_vf(double f) { lm_val v; v.f = f; return v; }
static inline lm_val lm_vp(void *p) { lm_val v; v.p = p; return v; }

/* ---- strings ---- */
static lm_str lm_str_new(const char *s, int64_t n, int l, int c) {
  LmStr *r = lm_alloc(sizeof(LmStr) + (size_t)n + 1, l, c);
  char *d = (char *)(r + 1);
  memcpy(d, s, (size_t)n); d[n] = 0;
  r->len = n; r->d = d; return r;
}
static lm_str lm_concat(int n, const lm_str *xs, int l, int c) { /* xs[0] + ... + xs[n-1] */
  int64_t len = 0;
  for (int i = 0; i < n; i++)
    if (__builtin_add_overflow(len, xs[i]->len, &len)) lm_panic(l, c, "memory limit exceeded");
  lm_charge(len, l, c);
  LmStr *r = lm_alloc(sizeof(LmStr) + (size_t)len + 1, l, c);
  char *d = (char *)(r + 1);
  r->len = len; r->d = d;
  for (int i = 0; i < n; i++) { memcpy(d, xs[i]->d, (size_t)xs[i]->len); d += xs[i]->len; }
  *d = 0;
  return r;
}
static int lm_str_cmp(lm_str a, lm_str b, int l, int c) {
  int64_t n = a->len < b->len ? a->len : b->len;
  lm_charge(n, l, c);
  int r = memcmp(a->d, b->d, (size_t)n);
  return r ? r : (a->len > b->len) - (a->len < b->len);
}
static bool lm_str_eq(lm_str a, lm_str b, int l, int c) {
  if (a->len != b->len) return false;
  lm_charge(a->len, l, c);
  return memcmp(a->d, b->d, (size_t)a->len) == 0;
}
static int64_t lm_str_len(lm_str s, int l, int c) { /* length in characters (UTF-8 code points) */
  lm_charge(s->len, l, c);
  int64_t n = 0;
  for (int64_t i = 0; i < s->len; i++) n += ((unsigned char)s->d[i] & 0xC0) != 0x80;
  return n;
}

/* ---- growable buffer used for formatting ---- */
typedef struct { char *d; int64_t len, cap; } lm_buf;
LM_G int lm_rl, lm_rc; /* location of the str()/print being formatted */
static void lm_put(lm_buf *b, const char *s, int64_t n) {
  if (b->len + n > b->cap) {
    int64_t old = b->cap;
    b->cap = (b->len + n) * 2 + 16;
    lm_mem_charge((size_t)(b->cap - old), lm_rl, lm_rc);
    b->d = realloc(b->d, (size_t)b->cap);
    if (!b->d) lm_panic(lm_rl, lm_rc, "out of memory");
  }
  memcpy(b->d + b->len, s, (size_t)n); b->len += n;
}
static void lm_puts(lm_buf *b, const char *s) { lm_put(b, s, (int64_t)strlen(s)); }
static void lm_fmt_float(char *out, double x) {
  if (isnan(x)) { strcpy(out, "nan"); return; }
  if (isinf(x)) { strcpy(out, x > 0 ? "inf" : "-inf"); return; }
  if (x == trunc(x) && fabs(x) < 1e16) { snprintf(out, 40, "%.1f", x); return; }
  for (int p = 1; p <= 17; p++) {
    snprintf(out, 40, "%.*g", p, x);
    if (strtod(out, NULL) == x) return;
  }
}
static lm_str lm_buf_str(lm_buf *b) {
  lm_str s = lm_str_new(b->d ? b->d : "", b->len, lm_rl, lm_rc);
  lm_mem -= b->cap; free(b->d); return s;
}

static void lm_struct_repr(lm_buf *b, int idx, void *p); /* generated per program */

static const char *lm_skip_desc(const char *d) {
  while (*d == 'L') d++;
  if (*d == 'S') { while (*d != ';') d++; }
  return d + 1;
}
/* desc: i f b s, L<elem>, S<idx>; */
static void lm_repr(lm_buf *b, lm_val v, const char **desc, bool quote) {
  char tmp[48];
  char k = *(*desc)++;
  /* Nesting (and cycles) are bounded by the stack check; output size by the
     step and memory budgets. */
  if (!LM_STACK_OK()) lm_panic(lm_rl, lm_rc, "value nested too deeply to format (cyclic?)");
  lm_charge(16, lm_rl, lm_rc);
  switch (k) {
  case 'i': snprintf(tmp, sizeof tmp, "%lld", (long long)v.i); lm_puts(b, tmp); break;
  case 'f': lm_fmt_float(tmp, v.f); lm_puts(b, tmp); break;
  case 'b': lm_puts(b, v.i ? "true" : "false"); break;
  case 's': {
    lm_str s = v.p;
    if (!quote) { lm_put(b, s->d, s->len); break; }
    lm_put(b, "\"", 1);
    for (int64_t i = 0; i < s->len; i++) {
      char ch = s->d[i];
      if (ch == '"' || ch == '\\') { lm_put(b, "\\", 1); lm_put(b, &ch, 1); }
      else if (ch == '\n') lm_puts(b, "\\n");
      else if (ch == '\t') lm_puts(b, "\\t");
      else lm_put(b, &ch, 1);
    }
    lm_put(b, "\"", 1);
    break;
  }
  case 'L': {
    lm_list *l = v.p;
    const char *start = *desc, *end = start;
    lm_put(b, "[", 1);
    for (int64_t i = 0; i < l->len; i++) {
      if (i) lm_puts(b, ", ");
      end = start;
      lm_repr(b, l->d[i], &end, true);
    }
    lm_put(b, "]", 1);
    end = lm_skip_desc(start);
    *desc = end;
    break;
  }
  case 'S': {
    int idx = (int)strtol(*desc, (char **)desc, 10);
    (*desc)++;
    lm_struct_repr(b, idx, v.p);
    break;
  }
  }
}
static lm_str lm_to_str(lm_val v, const char *desc, int l, int c) {
  lm_buf b = {0};
  lm_rl = l; lm_rc = c;
  lm_repr(&b, v, &desc, false);
  return lm_buf_str(&b);
}
static lm_str lm_str_int(int64_t i, int l, int c) {
  char t[24], *p = t + sizeof t;
  uint64_t u = i < 0 ? 0 - (uint64_t)i : (uint64_t)i;
  do { *--p = (char)('0' + u % 10); u /= 10; } while (u);
  if (i < 0) *--p = '-';
  return lm_str_new(p, t + sizeof t - p, l, c);
}
static lm_str lm_str_float(double f, int l, int c) { char t[48]; lm_fmt_float(t, f); return lm_str_new(t, (int64_t)strlen(t), l, c); }
static lm_str lm_str_bool(bool x, int l, int c) { return x ? lm_str_new("true", 4, l, c) : lm_str_new("false", 5, l, c); }

static void lm_print_val(lm_val v, const char *desc, int l, int c) {
  lm_buf b = {0};
  const char *d = desc;
  lm_rl = l; lm_rc = c;
  lm_repr(&b, v, &d, false);
  fwrite(b.d, 1, (size_t)b.len, stdout);
  lm_mem -= b.cap; free(b.d);
}
static void lm_print_int(int64_t i) { printf("%lld", (long long)i); }
static void lm_print_str(lm_str s, int l, int c) { lm_charge(s->len, l, c); fwrite(s->d, 1, (size_t)s->len, stdout); }

static int64_t lm_int_of_float(double f, int l, int c) {
  if (!(f >= -9223372036854775808.0 && f < 9223372036854775808.0)) lm_panic(l, c, "float value out of int range");
  return (int64_t)f;
}
/* Strict decimal syntax: optional sign, digits (and for floats '.' and an
   exponent); no whitespace, hex, inf or nan. */
static bool lm_num_syntax(const char *p, bool fl) {
  if (*p == '-' || *p == '+') p++;
  bool dig = false;
  while (*p >= '0' && *p <= '9') { p++; dig = true; }
  if (fl && *p == '.') { p++; while (*p >= '0' && *p <= '9') { p++; dig = true; } }
  if (!dig) return false;
  if (fl && (*p == 'e' || *p == 'E')) {
    p++;
    if (*p == '-' || *p == '+') p++;
    if (!(*p >= '0' && *p <= '9')) return false;
    while (*p >= '0' && *p <= '9') p++;
  }
  return *p == 0;
}
static int64_t lm_int_of_str(lm_str s, int l, int c) {
  char *end; const char *p = s->d;
  errno = 0;
  long long v = strtoll(p, &end, 10);
  if (!lm_num_syntax(p, false) || (int64_t)strlen(p) != s->len) lm_panic(l, c, "invalid int literal");
  if (errno == ERANGE) lm_panic(l, c, "int literal out of range");
  return (int64_t)v;
}
static double lm_float_of_str(lm_str s, int l, int c) {
  char *end; const char *p = s->d;
  double v = strtod(p, &end);
  if (!lm_num_syntax(p, true) || (int64_t)strlen(p) != s->len) lm_panic(l, c, "invalid float literal");
  return v;
}
/* ---- lists ---- */
static lm_list *lm_list_lit(int64_t n, const lm_val *items, int ln, int c) {
  lm_list *l = lm_alloc(sizeof(lm_list), ln, c);
  l->len = n; l->cap = n < 4 ? 4 : n;
  l->d = lm_xmalloc(sizeof(lm_val) * (size_t)l->cap, ln, c);
  if (n) memcpy(l->d, items, sizeof(lm_val) * (size_t)n);
  return l;
}
LM_COLD static void lm_oob(int64_t i, int64_t n, int ln, int c) {
  char m[96]; snprintf(m, sizeof m, "index %lld out of bounds for list of length %lld", (long long)i, (long long)n);
  lm_panic(ln, c, m);
}
static inline __attribute__((always_inline)) lm_val lm_get(lm_list *l, int64_t i, int ln, int c) {
  if (__builtin_expect((uint64_t)i >= (uint64_t)l->len, 0)) lm_oob(i, l->len, ln, c);
  return l->d[i];
}
static inline __attribute__((always_inline)) void lm_set(lm_list *l, int64_t i, lm_val v, int ln, int c) {
  if (__builtin_expect((uint64_t)i >= (uint64_t)l->len, 0)) lm_oob(i, l->len, ln, c);
  l->d[i] = v;
}
__attribute__((noinline)) static void lm_grow(lm_list *l, int ln, int c) {
  lm_mem_charge(sizeof(lm_val) * (size_t)(l->cap + 4), ln, c);
  l->cap = l->cap * 2 + 4;
  l->d = realloc(l->d, sizeof(lm_val) * (size_t)l->cap);
  if (!l->d) lm_panic(ln, c, "out of memory");
}
static inline void lm_push(lm_list *l, lm_val v, int ln, int c) {
  if (__builtin_expect(l->len == l->cap, 0)) lm_grow(l, ln, c);
  l->d[l->len++] = v;
}
static inline lm_val lm_pop(lm_list *l, int ln, int c) {
  if (__builtin_expect(l->len == 0, 0)) lm_panic(ln, c, "pop from empty list");
  return l->d[--l->len];
}
