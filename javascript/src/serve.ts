/**
 * An end-point on a socket.
 *
 * **There is no HTTP code here, and that is the point.** The component exports
 * `wasi:http/handler@0.3.0` and answers ECMA-431 §6 itself — the path, the method, the
 * body limit, the status line, `Retry-After`, the content type, the correlator header
 * and the CORS rule are all inside it, over `nlipsy-http`'s own values. What this file
 * does is hand that export whole to a runtime that can put it on a socket, which for
 * JavaScript today means `HttpServer` from `@bytecodealliance/preview3-shim/http`. Every
 * option a deployment has over the binding travels through `Endpoint.http` and is read
 * by the guest; none of it is read here.
 *
 * **Node only.** The shim's `HttpServer` is in its Node build alone: Bun fails on the
 * `worker_threads` transfer list the shim needs, and an incoming HTTP handler is not
 * something a browser can serve at all. The import is therefore dynamic, so that a
 * browser bundle that never calls this function does not pull a module that has no
 * `HttpServer` in it.
 *
 * **And it does not answer a request yet.** The listener starts; the first request traps
 * inside jco's glue the moment the guest reads the request's path. See the README's
 * "What runs today, and what does not".
 */

import { handler } from './glue.js';
import { createEndpoint } from './endpoint.js';
import type { InstalledEndpoint } from './endpoint.js';
import type { Endpoint } from './types.js';

/** Where to listen, and what to answer with. */
export interface ServeOptions extends Endpoint {
  /** The TCP port. */
  port: number;
  /** The interface. `127.0.0.1` by default: a server nobody asked to publish. */
  host?: string;
}

/** A listening end-point. */
export interface Server {
  /** The port it was asked for. */
  readonly port: number;
  /** The interface it was asked for. */
  readonly host: string;
  /** The end-point behind it, for its resolved configuration. */
  readonly endpoint: InstalledEndpoint;
  /** Stop listening and release the deployment slot. */
  close(): Promise<void>;
}

/** Minimal shape of the shim's server, so that this file needs no `any`. */
interface ShimServer {
  listen(port: number, host?: string): Promise<void>;
  stop(): Promise<void>;
  close(): Promise<void>;
}

/**
 * Install the end-point and put the component's own HTTP binding on a socket.
 *
 * @throws AlreadyInstalledError when a deployment is already installed.
 */
export async function serve(options: ServeOptions): Promise<Server> {
  const { port, host = '127.0.0.1', ...rest } = options;
  const endpoint = await createEndpoint(rest);
  let server: ShimServer;
  try {
    const shim = (await import('@bytecodealliance/preview3-shim/http')) as unknown as {
      HttpServer: new (handler: unknown) => ShimServer;
    };
    server = new shim.HttpServer(handler);
    await server.listen(port, host);
  } catch (error) {
    endpoint.close();
    throw error;
  }

  return {
    port,
    host,
    endpoint,
    async close(): Promise<void> {
      try {
        await server.close();
      } finally {
        endpoint.close();
      }
    },
  };
}
