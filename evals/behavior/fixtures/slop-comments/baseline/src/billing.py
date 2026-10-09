"""Billing helpers."""


def apply_discount(price, pct):
    return price * (1 - pct / 100)
