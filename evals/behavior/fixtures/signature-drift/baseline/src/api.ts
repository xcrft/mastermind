// User API — fixture for signature-drift auditor eval.

export interface User {
  id: string;
  name: string;
}

// fetchUser retrieves a user record by id.
// Pre-edit snapshot: fetchUser(id: string): Promise<User> — 3 callers.
export async function fetchUser(id: string): Promise<User> {
  return { id, name: "stub" };
}

export async function getProfile(id: string): Promise<User> {
  return fetchUser(id);
}

export async function refreshSession(id: string): Promise<User> {
  return fetchUser(id);
}

export async function authCheck(id: string): Promise<User> {
  return fetchUser(id);
}
