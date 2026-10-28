# Deal 4 hands of 5 cards from a deck shuffled with a seeded LCG (Fisher-Yates), then score high cards.
seed = [2024]

def rnd(n):
    seed[0] = (seed[0] * 1103515245 + 12345) % 2147483648
    return (seed[0] // 65536) % n

def shuffle(deck):
    for i in range(len(deck) - 1, 0, -1):
        j = rnd(i + 1)
        deck[i], deck[j] = deck[j], deck[i]

ranks = ["2", "3", "4", "5", "6", "7", "8", "9", "T", "J", "Q", "K", "A"]
suits = ["c", "d", "h", "s"]
deck = [r + s for s in suits for r in ranks]
shuffle(deck)
for h in range(4):
    hand = deck[h * 5:(h + 1) * 5]
    points = sum(max(0, ranks.index(c[0]) - 8) for c in hand)
    print("hand", h + 1, " ".join(hand), "points", points)
print("left", len(deck) - 20)
