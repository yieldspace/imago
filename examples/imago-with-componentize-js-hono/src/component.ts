import { Hono } from "hono";
import { fire } from "@bytecodealliance/jco-std/wasi/0.2.x/http/adapters/hono/server";

const app = new Hono();

app.get("/hello", (c) => {
    return c.json({ message: "Hello from componentize-js + Hono on imago!" });
});

fire(app);

export { incomingHandler } from "@bytecodealliance/jco-std/wasi/0.2.x/http/adapters/hono/server";
