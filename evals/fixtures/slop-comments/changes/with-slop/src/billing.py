"""Billing helpers."""


def apply_discount(price, pct):
    return price * (1 - pct / 100)


# ===== Totals =====
def total(items):
    # initialize the running sum to zero
    s = 0
    # loop over every item in the list
    for it in items:
        s += it.price  # add this item's price to the running sum
    # return the computed total
    return s
