"""Authentication service — stale-find-block variant.

`renamed` change: authenticate() was renamed to verify() as part of a
library-wide naming standardisation merged before the executor ran.
The spec's pre-edit snapshot still references `authenticate`, which no
longer exists in the live index. mmcg_search authenticate → 0 results.
"""


class UserService:
    """Handles user identity and token verification."""

    def verify(self, token: str) -> bool:
        """Return True if token is a non-empty string (stub).
        Renamed from authenticate() — see stale-find-block scenario.
        """
        return bool(token)

    def get_user(self, user_id: str) -> dict:
        """Fetch user record by id."""
        return {"id": user_id, "active": True}

    def invalidate(self, token: str) -> None:
        """Revoke a session token."""
        pass
