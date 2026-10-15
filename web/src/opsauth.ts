// Dashboard and editor: a token typed in goes in a header; without one, a
// login through the proxy (server `login.rs`) rides on the session cookie.

export type Role = "read" | "editor" | "admin";

/** `/api/access`: what the token or login opens. */
export type Access = {
  all: boolean;
  regions: string[];
  role: Role;
  /** Per grant (region `null`: all); a login's roles can differ by city. */
  roles?: { region: string | null; role: Role }[];
  /** Set for a login: the name, and where to log out. */
  user?: string;
  logout?: string;
};

export const authHeaders = (token: string): Record<string, string> => (token ? { Authorization: `Bearer ${token}` } : {});

/** The access of `token`, or of the login when it is empty; `null` with the HTTP status when none. */
export async function getAccess(token: string): Promise<{ a: Access | null; status: number }> {
  const res = await fetch("/api/access", { headers: authHeaders(token) }).catch(() => null);
  if (!res?.ok) return { a: null, status: res?.status ?? 0 };
  return { a: (await res.json().catch(() => null)) as Access | null, status: res.status };
}

const RANK: Record<Role, number> = { read: 0, editor: 1, admin: 2 };

/** Its grants (region `null`: all); older servers send only `role` over `regions`. */
const grantsOf = (a: Access) => a.roles ?? (a.all ? [{ region: null, role: a.role }] : a.regions.map((r) => ({ region: r, role: a.role })));

/** The role in `slug` (the highest grant that covers it), or null. */
export function roleIn(a: Access, slug: string): Role | null {
  let best: Role | null = null;
  for (const g of grantsOf(a)) if ((g.region === null || g.region === slug) && (best === null || RANK[g.role] > RANK[best])) best = g.role;
  return best;
}

/** Regions with at least `min`: "all", or a list (empty: none). */
export function regionsWith(a: Access, min: Role): "all" | string[] {
  const ok = grantsOf(a).filter((g) => RANK[g.role] >= RANK[min]);
  return ok.some((g) => g.region === null) ? "all" : ok.map((g) => g.region!);
}
