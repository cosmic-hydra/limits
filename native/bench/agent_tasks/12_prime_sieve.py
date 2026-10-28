# Sieve of Eratosthenes.
n = 2000000
comp = bytearray(n + 1)
primes = []
for i in range(2, n + 1):
    if not comp[i]:
        primes.append(i)
        comp[i * i :: i] = b"\x01" * len(range(i * i, n + 1, i))
print(len(primes), sum(primes), primes[-1])
