/* TinyCC lacks __builtin_{add,sub,mul}_overflow; every use in the generated
   C is on int64_t, so these non-generic replacements suffice (from
   tools/tcc_shim.h). Emitted only when compiling with tcc. */
#include <stdint.h>
static inline __attribute__((unused)) int lm_tcc_add_ovf(int64_t a, int64_t b, int64_t *r) {
  *r = (int64_t)((uint64_t)a + (uint64_t)b);
  return (b > 0 && a > INT64_MAX - b) || (b < 0 && a < INT64_MIN - b);
}
static inline __attribute__((unused)) int lm_tcc_sub_ovf(int64_t a, int64_t b, int64_t *r) {
  *r = (int64_t)((uint64_t)a - (uint64_t)b);
  return (b < 0 && a > INT64_MAX + b) || (b > 0 && a < INT64_MIN + b);
}
static inline __attribute__((unused)) int lm_tcc_mul_ovf(int64_t a, int64_t b, int64_t *r) {
  *r = (int64_t)((uint64_t)a * (uint64_t)b);
  if (a == 0 || b == 0) return 0;
  if (a == -1) return b == INT64_MIN;
  if (b == -1) return a == INT64_MIN;
  return a > 0 ? (b > 0 ? a > INT64_MAX / b : b < INT64_MIN / a)
               : (b > 0 ? a < INT64_MIN / b : a < INT64_MAX / b);
}
#define __builtin_add_overflow(a, b, r) lm_tcc_add_ovf((a), (b), (r))
#define __builtin_sub_overflow(a, b, r) lm_tcc_sub_ovf((a), (b), (r))
#define __builtin_mul_overflow(a, b, r) lm_tcc_mul_ovf((a), (b), (r))
