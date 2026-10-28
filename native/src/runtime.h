/* limits runtime: embedded verbatim into every generated C file. */
#define _GNU_SOURCE /* memmem */
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <errno.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <fcntl.h>
#include <limits.h>
#include <time.h>
#include <unistd.h>

typedef struct { int64_t len; const char *d; } LmStr;
typedef LmStr *lm_str;
typedef union { int64_t i; double f; void *p; } lm_val;
typedef struct { int64_t len, cap; void *d; } lm_list;

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
/* Not every program uses every runtime helper. */
#define LM_U __attribute__((unused))
#define LM_RT static LM_U
LM_G LM_U int64_t lm_steps, lm_depth, lm_mem;
LM_G char *lm_ap, *lm_aend, *lm_stack_lo;

/* Source locations: line | file index << 20 (file 0 is the root file). */
#define LM_FILE_SHIFT 20
LM_RT const char *lm_file_name(int i); /* generated per program */
#define LM_COLD __attribute__((noreturn, cold, noinline, unused))
#ifdef LM_JSON
LM_RT void lm_json_str(const char *s) {
  fputc('"', stderr);
  for (; *s; s++) {
    unsigned char ch = (unsigned char)*s;
    if (ch == '"' || ch == '\\') fprintf(stderr, "\\%c", ch);
    else if (ch < 0x20) fprintf(stderr, "\\u%04x", ch);
    else if (ch < 0x80) fputc(ch, stderr);
    else { /* copy a valid UTF-8 sequence; replace a bad byte with U+FFFD */
      int n = ch < 0xC2 ? -1 : ch < 0xE0 ? 1 : ch < 0xF0 ? 2 : ch < 0xF5 ? 3 : -1;
      int k = 1;
      while (k <= n && ((unsigned char)s[k] & 0xC0) == 0x80) k++;
      unsigned char c1 = (unsigned char)s[1];
      bool bad = n < 0 || k <= n || (ch == 0xE0 && c1 < 0xA0) || (ch == 0xED && c1 >= 0xA0) ||
                 (ch == 0xF0 && c1 < 0x90) || (ch == 0xF4 && c1 >= 0x90);
      if (bad) fputs("\\ufffd", stderr);
      else { fwrite(s, 1, (size_t)k, stderr); s += n; }
    }
  }
  fputc('"', stderr);
}
#endif
LM_COLD static void lm_panic(int l, int c, const char *m) {
  fflush(stdout);
  int f = l >> LM_FILE_SHIFT;
#ifdef LM_JSON
  const char *code = "E0500";
  for (int i = 0; lm_codes[i]; i += 2)
    if (strstr(m, lm_codes[i])) { code = lm_codes[i + 1]; break; }
  fprintf(stderr, "{\"kind\":\"runtime\",\"code\":\"%s\",\"file\":", code);
  lm_json_str(lm_file_name(f));
  fprintf(stderr, ",\"line\":%d,\"col\":%d,\"msg\":", l & ((1 << LM_FILE_SHIFT) - 1), c);
  lm_json_str(m);
  fputs(",\"hint\":\"\"}\n", stderr);
  exit(1);
#endif
  if (f) fprintf(stderr, "runtime error in %s at %d:%d: %s\n", lm_file_name(f), l & ((1 << LM_FILE_SHIFT) - 1), c, m);
  else fprintf(stderr, "runtime error at %d:%d: %s\n", l, c, m);
  exit(1);
}
/* The C stack is checked directly (not only the frame count), so neither large
   frames nor deep runtime recursion (printing) can overflow it. */
LM_RT void lm_stack_init(void) {
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
LM_RT inline void lm_charge(int64_t n, int l, int c) {
  if (__builtin_expect((lm_steps -= n >> 6) < 0, 0)) lm_panic(l, c, "step limit exceeded");
}
#define LM_ARITH(name, op, bi) \
  LM_RT inline int64_t name(int64_t a, int64_t b, int l, int c) { \
    int64_t r; if (__builtin_expect(bi(a, b, &r), 0)) lm_panic(l, c, "integer overflow in '" op "'"); return r; }
#else
#define LM_STEP(l, c) ((void)0)
#define LM_ENTER(l, c) ((void)0)
#define LM_LEAVE ((void)0)
#define LM_CALL(T, call) (call)
#define LM_CALLV(call) (call)
#define lm_charge(n, l, c) ((void)(n), (void)(l), (void)(c))
#define LM_ARITH(name, op, bi) \
  LM_RT inline int64_t name(int64_t a, int64_t b, int l, int c) { \
    (void)l; (void)c; int64_t r; bi(a, b, &r); return r; }
#endif
LM_ARITH(lm_add, "+", __builtin_add_overflow)
LM_ARITH(lm_sub, "-", __builtin_sub_overflow)
LM_ARITH(lm_mul, "*", __builtin_mul_overflow)

LM_RT inline int64_t lm_neg(int64_t a, int l, int c) { return lm_sub(0, a, l, c); }
LM_RT inline int64_t lm_div(int64_t a, int64_t b, int l, int c) {
  if (__builtin_expect(b == 0, 0)) lm_panic(l, c, "division by zero");
#ifdef LM_CHECKED
  if (__builtin_expect(b == -1 && a == INT64_MIN, 0)) lm_panic(l, c, "integer overflow in '/'");
#endif
  if (b == -1) return (int64_t)(0 - (uint64_t)a);
  return a / b;
}
LM_RT inline int64_t lm_mod(int64_t a, int64_t b, int l, int c) {
  if (__builtin_expect(b == 0, 0)) lm_panic(l, c, "division by zero");
  if (b == -1) return 0;
  return a % b;
}

/* Bump arena: memory is never freed (v0.1 limitation). Every heap byte is
   charged against the --max-mem budget (all modes). */
LM_RT void lm_mem_charge(size_t n, int l, int c) {
  if (n > (size_t)INT64_MAX || (lm_mem += (int64_t)n) > LM_MAX_MEM) lm_panic(l, c, "memory limit exceeded");
}
LM_RT void *lm_xmalloc(size_t n, int l, int c) {
  lm_mem_charge(n, l, c);
  void *p = malloc(n);
  if (!p) lm_panic(l, c, "out of memory");
  return p;
}
LM_RT void *lm_alloc(size_t n, int l, int c) {
  n = (n + 15) & ~(size_t)15;
  if (n > 65536) return lm_xmalloc(n, l, c);
  /* Charge the bytes handed out, not the 1 MiB chunk, so --max-mem 1 works
     and the error points at the allocation that crosses the budget. */
  lm_mem_charge(n, l, c);
  if (n > (size_t)(lm_aend - lm_ap)) {
    lm_ap = malloc(1 << 20);
    if (!lm_ap) lm_panic(l, c, "out of memory");
    lm_aend = lm_ap + (1 << 20);
  }
  void *p = lm_ap; lm_ap += n; return p;
}

LM_RT inline lm_val lm_vi(int64_t i) { lm_val v; v.i = i; return v; }
LM_RT inline lm_val lm_vf(double f) { lm_val v; v.f = f; return v; }
LM_RT inline lm_val lm_vp(void *p) { lm_val v; v.p = p; return v; }

/* ---- strings ---- */
LM_RT lm_str lm_str_new(const char *s, int64_t n, int l, int c) {
  LmStr *r = lm_alloc(sizeof(LmStr) + (size_t)n + 1, l, c);
  char *d = (char *)(r + 1);
  memcpy(d, s, (size_t)n); d[n] = 0;
  r->len = n; r->d = d; return r;
}
LM_RT lm_str lm_concat(int n, const lm_str *xs, int l, int c) { /* xs[0] + ... + xs[n-1] */
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
LM_RT int lm_str_cmp(lm_str a, lm_str b, int l, int c) {
  int64_t n = a->len < b->len ? a->len : b->len;
  lm_charge(n, l, c);
  int r = memcmp(a->d, b->d, (size_t)n);
  return r ? r : (a->len > b->len) - (a->len < b->len);
}
LM_RT bool lm_str_eq(lm_str a, lm_str b, int l, int c) {
  if (a->len != b->len) return false;
  lm_charge(a->len, l, c);
  return memcmp(a->d, b->d, (size_t)a->len) == 0;
}
LM_RT int64_t lm_str_len(lm_str s, int l, int c) { /* length in characters (UTF-8 code points) */
  lm_charge(s->len, l, c);
  int64_t n = 0;
  for (int64_t i = 0; i < s->len; i++) n += ((unsigned char)s->d[i] & 0xC0) != 0x80;
  return n;
}

/* ---- growable buffer used for formatting ---- */
typedef struct { char *d; int64_t len, cap; } lm_buf;
LM_G int lm_rl, lm_rc; /* location of the str()/print being formatted */
LM_RT void lm_put(lm_buf *b, const char *s, int64_t n) {
  if (b->len + n > b->cap) {
    int64_t old = b->cap;
    b->cap = (b->len + n) * 2 + 16;
    lm_mem_charge((size_t)(b->cap - old), lm_rl, lm_rc);
    b->d = realloc(b->d, (size_t)b->cap);
    if (!b->d) lm_panic(lm_rl, lm_rc, "out of memory");
  }
  memcpy(b->d + b->len, s, (size_t)n); b->len += n;
}
LM_RT void lm_puts(lm_buf *b, const char *s) { lm_put(b, s, (int64_t)strlen(s)); }
#define LM_FLT_MAX 400 /* longest positional double: 5e-324 or DBL_MAX */
LM_RT void lm_fmt_float(char *out, double x) {
  if (isnan(x)) { strcpy(out, "nan"); return; }
  if (isinf(x)) { strcpy(out, x > 0 ? "inf" : "-inf"); return; }
  if (x == trunc(x) && fabs(x) < 1e16) { snprintf(out, 40, "%.1f", x); return; }
  /* Shortest round-tripping digits, written positionally (Limits has no
     exponent literals, so printed floats can be pasted back as source). */
  char e[40], dig[24], *o = out;
  int p = 1, nd = 0;
  for (; p < 17; p++) {
    snprintf(e, sizeof e, "%.*e", p - 1, x);
    if (strtod(e, NULL) == x) break;
  }
  snprintf(e, sizeof e, "%.*e", p - 1, x);
  const char *q = e;
  if (*q == '-') { *o++ = '-'; q++; }
  for (; *q != 'e'; q++) if (*q != '.') dig[nd++] = *q;
  while (nd > 1 && dig[nd - 1] == '0') nd--;
  int ex = atoi(q + 1); /* value = 0.dig * 10^(ex+1) */
  if (ex < 0) {
    o += sprintf(o, "0.");
    for (int i = -1; i > ex; i--) *o++ = '0';
    memcpy(o, dig, (size_t)nd); o += nd;
  } else {
    for (int i = 0; i <= ex || i < nd; i++) {
      if (i == ex + 1) *o++ = '.';
      *o++ = i < nd ? dig[i] : '0';
    }
    if (nd <= ex + 1) o += sprintf(o, ".0");
  }
  *o = 0;
}
LM_RT lm_str lm_buf_str(lm_buf *b) {
  lm_str s = lm_str_new(b->d ? b->d : "", b->len, lm_rl, lm_rc);
  lm_mem -= b->cap; free(b->d); return s;
}

LM_RT void lm_struct_repr(lm_buf *b, int idx, void *p); /* generated per program */

LM_RT const char *lm_skip_desc(const char *d) {
  while (*d == 'L') d++;
  if (*d == 'S') { while (*d != ';') d++; }
  return d + 1;
}
/* desc: i f b s, L<elem>, S<idx>; */
LM_RT void lm_repr(lm_buf *b, lm_val v, const char **desc, bool quote) {
  char tmp[LM_FLT_MAX];
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
      lm_val e;
      if (*start == 'b') e.i = ((bool *)l->d)[i];
      else memcpy(&e, (char *)l->d + 8 * i, 8);
      lm_repr(b, e, &end, true);
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
LM_RT lm_str lm_to_str(lm_val v, const char *desc, int l, int c) {
  lm_buf b = {0};
  lm_rl = l; lm_rc = c;
  lm_repr(&b, v, &desc, false);
  return lm_buf_str(&b);
}
LM_RT lm_str lm_str_int(int64_t i, int l, int c) {
  char t[24], *p = t + sizeof t;
  uint64_t u = i < 0 ? 0 - (uint64_t)i : (uint64_t)i;
  do { *--p = (char)('0' + u % 10); u /= 10; } while (u);
  if (i < 0) *--p = '-';
  return lm_str_new(p, t + sizeof t - p, l, c);
}
LM_RT lm_str lm_str_float(double f, int l, int c) { char t[LM_FLT_MAX]; lm_fmt_float(t, f); return lm_str_new(t, (int64_t)strlen(t), l, c); }
LM_RT lm_str lm_str_bool(bool x, int l, int c) { return x ? lm_str_new("true", 4, l, c) : lm_str_new("false", 5, l, c); }

/* print appends its arguments to lm_line and writes the line only once all of
   them are formatted, so a runtime error in an argument leaves no partial
   line. A print nested in an argument (via a call) uses the tail of the
   buffer past the outer print's start offset. */
LM_G lm_buf lm_line;
LM_RT void lm_print_val(lm_val v, const char *desc, int l, int c) {
  const char *d = desc;
  lm_rl = l; lm_rc = c;
  lm_repr(&lm_line, v, &d, false);
}
LM_RT void lm_print_int(int64_t i, int l, int c) {
  char t[24];
  lm_rl = l; lm_rc = c;
  lm_put(&lm_line, t, snprintf(t, sizeof t, "%lld", (long long)i));
}
LM_RT void lm_print_str(lm_str s, int l, int c) {
  lm_charge(s->len, l, c);
  lm_rl = l; lm_rc = c;
  lm_put(&lm_line, s->d, s->len);
}
LM_RT void lm_print_sep(char ch, int l, int c) { lm_rl = l; lm_rc = c; lm_put(&lm_line, &ch, 1); }
LM_RT void lm_print_end(int64_t start) {
  fwrite(lm_line.d + start, 1, (size_t)(lm_line.len - start), stdout);
  lm_line.len = start;
}

LM_RT int64_t lm_int_of_float(double f, int l, int c) {
  if (!(f >= -9223372036854775808.0 && f < 9223372036854775808.0)) lm_panic(l, c, "float value out of int range");
  return (int64_t)f;
}
/* Strict decimal syntax: optional sign, digits (and for floats '.' and an
   exponent); no whitespace, hex, inf or nan. */
LM_RT bool lm_num_syntax(const char *p, bool fl) {
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
LM_RT int64_t lm_int_of_str(lm_str s, int l, int c) {
  char *end; const char *p = s->d;
  errno = 0;
  long long v = strtoll(p, &end, 10);
  if (!lm_num_syntax(p, false) || (int64_t)strlen(p) != s->len) lm_panic(l, c, "invalid int literal");
  if (errno == ERANGE) lm_panic(l, c, "int literal out of range");
  return (int64_t)v;
}
LM_RT double lm_float_of_str(lm_str s, int l, int c) {
  char *end; const char *p = s->d;
  double v = strtod(p, &end);
  if (!lm_num_syntax(p, true) || (int64_t)strlen(p) != s->len) lm_panic(l, c, "invalid float literal");
  return v;
}
/* ---- lists ----
   Elements are packed by static type: [bool] 1 byte, [int]/[float] 8 bytes,
   everything else one pointer. LM_LIST(sfx, T) defines the typed accessors. */
LM_COLD static void lm_oob(int64_t i, int64_t n, int ln, int c) {
  char m[96]; snprintf(m, sizeof m, "index %lld out of bounds for list of length %lld", (long long)i, (long long)n);
  lm_panic(ln, c, m);
}
__attribute__((noinline)) LM_RT void lm_grow(lm_list *l, size_t esz, int ln, int c) {
  lm_mem_charge(esz * (size_t)(l->cap + 4), ln, c);
  l->cap = l->cap * 2 + 4;
  l->d = realloc(l->d, esz * (size_t)l->cap);
  if (!l->d) lm_panic(ln, c, "out of memory");
}
LM_RT lm_list *lm_list_new(int64_t n, const void *items, size_t esz, int ln, int c) {
  lm_list *l = lm_alloc(sizeof(lm_list), ln, c);
  l->len = n; l->cap = n < 4 ? 4 : n;
  l->d = lm_xmalloc(esz * (size_t)l->cap, ln, c);
  if (n) memcpy(l->d, items, esz * (size_t)n);
  return l;
}
#define LM_LIST(S, T) \
  LM_RT inline __attribute__((always_inline)) T lm_get_##S(lm_list *l, int64_t i, int ln, int c) { \
    if (__builtin_expect((uint64_t)i >= (uint64_t)l->len, 0)) lm_oob(i, l->len, ln, c); \
    return ((T *)l->d)[i]; } \
  LM_RT inline __attribute__((always_inline)) void lm_set_##S(lm_list *l, int64_t i, T v, int ln, int c) { \
    if (__builtin_expect((uint64_t)i >= (uint64_t)l->len, 0)) lm_oob(i, l->len, ln, c); \
    ((T *)l->d)[i] = v; } \
  LM_RT inline void lm_push_##S(lm_list *l, T v, int ln, int c) { \
    if (__builtin_expect(l->len == l->cap, 0)) lm_grow(l, sizeof(T), ln, c); \
    ((T *)l->d)[l->len++] = v; } \
  LM_RT inline T lm_pop_##S(lm_list *l, int ln, int c) { \
    if (__builtin_expect(l->len == 0, 0)) lm_panic(ln, c, "pop from empty list"); \
    return ((T *)l->d)[--l->len]; } \
  LM_RT inline lm_list *lm_lit_##S(int64_t n, const T *items, int ln, int c) { \
    return lm_list_new(n, items, sizeof(T), ln, c); }
LM_LIST(b, bool)
LM_LIST(i, int64_t)
LM_LIST(f, double)
typedef void *lm_ptr;
LM_LIST(p, lm_ptr)

/* ---- split / join / sort: one step per 64 bytes or per element pass ---- */
LM_RT lm_list *lm_split(lm_str s, lm_str sep, int l, int c) {
  lm_list *r = lm_list_new(0, NULL, sizeof(lm_ptr), l, c);
  lm_charge(s->len, l, c);
  int64_t i = 0, n = sep->len;
  for (;;) {
    int64_t j = i;
    if (n == 0) { /* empty separator: one string per code point */
      if (i == s->len) return r;
      for (j++; j < s->len && ((unsigned char)s->d[j] & 0xC0) == 0x80; j++) {}
    } else {
      const char *f = memmem(s->d + i, (size_t)(s->len - i), sep->d, (size_t)n);
      j = f ? f - s->d : s->len;
    }
    lm_charge(64, l, c);
    lm_push_p(r, lm_str_new(s->d + i, j - i, l, c), l, c);
    if (n && j == s->len) return r;
    i = j + n;
  }
}
LM_RT lm_str lm_join(lm_list *xs, lm_str sep, int l, int c) {
  lm_str *p = xs->d;
  int64_t len = 0;
  for (int64_t i = 0; i < xs->len; i++)
    if (__builtin_add_overflow(len, p[i]->len + (i ? sep->len : 0), &len)) lm_panic(l, c, "memory limit exceeded");
  lm_charge(len + (xs->len << 6), l, c);
  LmStr *r = lm_alloc(sizeof(LmStr) + (size_t)len + 1, l, c);
  char *d = (char *)(r + 1);
  r->len = len; r->d = d;
  for (int64_t i = 0; i < xs->len; i++) {
    if (i) { memcpy(d, sep->d, (size_t)sep->len); d += sep->len; }
    memcpy(d, p[i]->d, (size_t)p[i]->len); d += p[i]->len;
  }
  *d = 0;
  return r;
}
/* Stable bottom-up merge sort; the scratch buffer is charged to --max-mem. */
#define LM_SORT(S, T, LE) \
  LM_RT void lm_sort_##S(lm_list *xs, int l, int c) { \
    int64_t n = xs->len; \
    if (n < 2) return; \
    T *a = xs->d, *b = lm_xmalloc(sizeof(T) * (size_t)n, l, c), *t; \
    for (int64_t w = 1; w < n; w *= 2) { \
      lm_charge(n << 6, l, c); \
      for (int64_t lo = 0; lo < n; lo += 2 * w) { \
        int64_t mid = lo + w < n ? lo + w : n, hi = lo + 2 * w < n ? lo + 2 * w : n, i = lo, j = mid; \
        for (int64_t k = lo; k < hi; k++) b[k] = (j >= hi || (i < mid && LE(a[i], a[j]))) ? a[i++] : a[j++]; \
      } \
      t = a; a = b; b = t; \
    } \
    if (a != xs->d) { memcpy(xs->d, a, sizeof(T) * (size_t)n); b = a; } \
    free(b); \
  }
#define LM_LE_NUM(x, y) ((x) <= (y))
#define LM_LE_F(x, y) (isnan(y) || (x) <= (y)) /* NaN sorts last */
#define LM_LE_S(x, y) (lm_str_cmp((lm_str)(x), (lm_str)(y), l, c) <= 0)
LM_SORT(i, int64_t, LM_LE_NUM)
LM_SORT(f, double, LM_LE_F)
LM_SORT(p, lm_ptr, LM_LE_S)

/* ---- capability-gated I/O ----
   Paths are canonicalised (realpath: resolves '..' and symlinks) and must lie
   inside an allowed prefix (lm_allow_read/lm_allow_write, generated from
   --allow; an empty list means the current directory). The resolved path is
   then opened with O_NOFOLLOW, and only regular files are accepted. */
LM_G LM_U int lm_argc;
LM_G char **lm_argv;
LM_COLD static void lm_io_fail(const char *op, lm_str path, int l, int c) {
  const char *why = errno ? strerror(errno) : "not a regular file";
  char *m = malloc((size_t)path->len + 160);
  snprintf(m, (size_t)path->len + 160, "cannot %s file '%s': %s", op, path->d, why);
  lm_panic(l, c, m);
}
LM_RT bool lm_path_ok(const char *p, const char *const *allow) {
  char cwd[PATH_MAX];
  const char *here[2] = {cwd, 0};
  if (!*allow) {
    if (!realpath(".", cwd)) return false;
    allow = here;
  }
  for (; *allow; allow++) {
    size_t n = strlen(*allow);
    if (strncmp(p, *allow, n) == 0 && (p[n] == 0 || p[n] == '/' || (*allow)[n - 1] == '/')) return true;
  }
  return false;
}
/* Canonical path of `s` into `out`. A missing final component is allowed (its
   directory is canonicalised instead), so a missing file outside the sandbox
   is denied rather than reported as missing. */
LM_RT void lm_resolve(lm_str s, char *out, const char *const *allow, const char *op, int l, int c) {
  errno = EINVAL;
  if (s->len == 0 || s->len >= PATH_MAX || (int64_t)strlen(s->d) != s->len) lm_io_fail(op, s, l, c);
  if (!realpath(s->d, out)) {
    if (errno != ENOENT) lm_io_fail(op, s, l, c);
    char dir[PATH_MAX], rdir[PATH_MAX];
    memcpy(dir, s->d, (size_t)s->len + 1);
    char *slash = strrchr(dir, '/');
    const char *base = slash ? slash + 1 : dir, *d = ".";
    if (slash) { *slash = 0; d = slash == dir ? "/" : dir; }
    errno = EINVAL;
    if (!*base || !strcmp(base, ".") || !strcmp(base, "..")) lm_io_fail(op, s, l, c);
    if (!realpath(d, rdir)) lm_io_fail(op, s, l, c);
    errno = ENAMETOOLONG;
    if (snprintf(out, PATH_MAX, "%s/%s", strcmp(rdir, "/") ? rdir : "", base) >= PATH_MAX) lm_io_fail(op, s, l, c);
  }
  if (!lm_path_ok(out, allow)) {
    char *m = malloc((size_t)s->len + 160);
    snprintf(m, (size_t)s->len + 160, "access denied: '%s' is outside the allowed fs.%s paths (see --allow)", s->d, op);
    lm_panic(l, c, m);
  }
}
LM_RT int lm_open(lm_str path, int flags, const char *const *allow, const char *op, int l, int c) {
  char p[PATH_MAX];
  lm_resolve(path, p, allow, op, l, c);
  /* O_NONBLOCK: opening a FIFO or device must not block (it is then
     rejected as not a regular file); regular files ignore the flag. */
  struct stat st;
  if (stat(p, &st) == 0 && !S_ISREG(st.st_mode)) { errno = 0; lm_io_fail(op, path, l, c); }
  int fd = open(p, flags | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK, 0666);
  if (fd < 0) lm_io_fail(op, path, l, c);
  if (fstat(fd, &st) || !S_ISREG(st.st_mode)) { errno = 0; close(fd); lm_io_fail(op, path, l, c); }
  return fd;
}
LM_RT lm_str lm_read_file(lm_str path, int l, int c) {
  int fd = lm_open(path, O_RDONLY, lm_allow_read, "read", l, c);
  lm_buf b = {0, 0, 0};
  char t[65536];
  ssize_t n;
  lm_rl = l; lm_rc = c;
  while ((n = read(fd, t, sizeof t)) > 0) { lm_charge(n, l, c); lm_put(&b, t, n); }
  if (n < 0) lm_io_fail("read", path, l, c);
  close(fd);
  return lm_buf_str(&b);
}
LM_RT void lm_write_file(lm_str path, lm_str data, int l, int c) {
  /* Bytes written count against the memory budget (bounds disk use too). */
  lm_mem_charge((size_t)data->len, l, c);
  lm_charge(data->len, l, c);
  int fd = lm_open(path, O_WRONLY | O_CREAT | O_TRUNC, lm_allow_write, "write", l, c);
  for (int64_t off = 0; off < data->len;) {
    ssize_t n = write(fd, data->d + off, (size_t)(data->len - off));
    if (n < 0 && errno == EINTR) continue;
    if (n <= 0) lm_io_fail("write", path, l, c);
    off += n;
  }
  if (close(fd)) lm_io_fail("write", path, l, c);
}
LM_RT lm_str lm_env(lm_str name, int l, int c) {
  bool ok = !lm_allow_env[0];
  for (const char *const *a = lm_allow_env; *a; a++) ok |= !strcmp(*a, name->d);
  if (!ok) {
    char *m = malloc((size_t)name->len + 96);
    snprintf(m, (size_t)name->len + 96, "access denied: env var '%s' is not allowed (see --allow)", name->d);
    lm_panic(l, c, m);
  }
  const char *v = (int64_t)strlen(name->d) == name->len ? getenv(name->d) : NULL;
  return v ? lm_str_new(v, (int64_t)strlen(v), l, c) : lm_str_new("", 0, l, c);
}
LM_RT int64_t lm_now_ms(void) {
  struct timespec ts;
  clock_gettime(CLOCK_REALTIME, &ts);
  return (int64_t)ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}
LM_RT lm_list *lm_args(int l, int c) {
  lm_list *r = lm_list_new(0, NULL, sizeof(lm_ptr), l, c);
  for (int i = 1; i < lm_argc; i++) lm_push_p(r, lm_str_new(lm_argv[i], (int64_t)strlen(lm_argv[i]), l, c), l, c);
  return r;
}
LM_RT lm_str lm_input_line(int l, int c) { /* one line from stdin without '\n'; "" at EOF */
  lm_buf b = {0, 0, 0};
  char t[4096];
  int ch = 0;
  lm_rl = l; lm_rc = c;
  fflush(stdout);
  while (ch != '\n' && ch != EOF) { /* bytes are kept as-is, NUL included */
    int64_t n = 0;
    while (n < (int64_t)sizeof t && (ch = getchar()) != EOF && ch != '\n') t[n++] = (char)ch;
    lm_charge(n, l, c);
    lm_put(&b, t, n);
  }
  return lm_buf_str(&b);
}
