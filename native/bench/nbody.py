import math
class Body:
    __slots__ = ("x", "y", "z", "vx", "vy", "vz", "m")
    def __init__(s, x, y, z, vx, vy, vz, m):
        s.x, s.y, s.z, s.vx, s.vy, s.vz, s.m = x, y, z, vx, vy, vz, m

def energy(bs):
    e = 0.0
    n = len(bs)
    for i in range(n):
        b = bs[i]
        e += 0.5 * b.m * (b.vx * b.vx + b.vy * b.vy + b.vz * b.vz)
        for j in range(i + 1, n):
            c = bs[j]
            dx = b.x - c.x; dy = b.y - c.y; dz = b.z - c.z
            e -= b.m * c.m / math.sqrt(dx * dx + dy * dy + dz * dz)
    return e

def advance(bs, dt):
    n = len(bs)
    for i in range(n):
        b = bs[i]
        for j in range(i + 1, n):
            c = bs[j]
            dx = b.x - c.x; dy = b.y - c.y; dz = b.z - c.z
            d2 = dx * dx + dy * dy + dz * dz
            mag = dt / (d2 * math.sqrt(d2))
            b.vx -= dx * c.m * mag; b.vy -= dy * c.m * mag; b.vz -= dz * c.m * mag
            c.vx += dx * b.m * mag; c.vy += dy * b.m * mag; c.vz += dz * b.m * mag
    for b in bs:
        b.x += dt * b.vx; b.y += dt * b.vy; b.z += dt * b.vz

pi = 3.141592653589793
sm = 4.0 * pi * pi
dp = 365.24
bs = [
    Body(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, sm),
    Body(4.84143144246472090, -1.16032004402742839, -0.103622044471123109,
         0.00166007664274403694 * dp, 0.00769901118419740425 * dp, -0.0000690460016972063023 * dp,
         0.000954791938424326609 * sm),
    Body(8.34336671824457987, 4.12479856412430479, -0.403523417114321381,
         -0.00276742510726862411 * dp, 0.00499852801234917238 * dp, 0.0000230417297573763929 * dp,
         0.000285885980666130812 * sm),
    Body(12.8943695621391310, -15.1111514016986312, -0.223307578892655734,
         0.00296460137564761618 * dp, 0.00237847173959480950 * dp, -0.0000296589568540237556 * dp,
         0.0000436624404335156298 * sm),
    Body(15.3796971148509165, -25.9193146099879641, 0.179258772950371181,
         0.00268067772490389322 * dp, 0.00162824170038242295 * dp, -0.0000951592254519715870 * dp,
         0.0000515138902046611451 * sm),
]
px = py = pz = 0.0
for b in bs:
    px += b.vx * b.m; py += b.vy * b.m; pz += b.vz * b.m
s = bs[0]
s.vx = 0.0 - px / sm; s.vy = 0.0 - py / sm; s.vz = 0.0 - pz / sm
print(energy(bs))
for k in range(5000000):
    advance(bs, 0.01)
print(energy(bs))
