// Minimal Express router — scope-creep variant.
// Executor added the /health route per spec. Also modified (scope creep).
import { Router, Request, Response } from "express";

const router = Router();

router.get("/users", (_req: Request, res: Response) => {
  res.json({ users: [] });
});

router.post("/users", (req: Request, res: Response) => {
  res.status(201).json({ id: "new", ...req.body });
});

// Added per spec — this was the only intended change.
router.get("/health", (_req: Request, res: Response) => {
  res.json({ status: "ok" });
});

export default router;
