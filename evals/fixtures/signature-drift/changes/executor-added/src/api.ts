// User API.

export interface User {
  id: string;
  name: string;
}

export interface FetchOptions {
  timeout: number;
}

export async function fetchUser(
  id: string,
  options: FetchOptions
): Promise<User> {
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
