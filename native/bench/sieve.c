#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <stdbool.h>
int main(void) {
  int64_t n = 10000000, count = 0;
  bool *is = malloc((size_t)n + 1);
  for (int64_t i = 0; i <= n; i++) is[i] = true;
  for (int64_t i = 2; i <= n; i++)
    if (is[i]) { count++; for (int64_t j = i * i; j <= n; j += i) is[j] = false; }
  printf("%lld\n", (long long)count);
  return 0;
}
