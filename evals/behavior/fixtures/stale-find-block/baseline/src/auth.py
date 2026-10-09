"""Authentication service — fixture for stale-find-block auditor eval."""


class UserService:
    """Handles user identity and token verification."""

    def authenticate(self, token: str) -> bool:
        """Return True if token is a non-empty string (stub)."""
        return bool(token)

    def get_user(self, user_id: str) -> dict:
        """Fetch user record by id."""
        return {"id": user_id, "active": True}

    def invalidate(self, token: str) -> None:
        """Revoke a session token."""
        pass
