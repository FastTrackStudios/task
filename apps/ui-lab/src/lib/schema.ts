/**
 * Proto/server schema-skew guard — the TS mirror of
 * `org_proto::schema_stamp` (see `task doctor` for the CLI side).
 *
 * A vox method id hashes the method's name + payload shapes, so
 * XOR-folding a service's method ids gives a stamp that changes
 * whenever any verb is added/removed or any payload shape is
 * touched. The server publishes its stamps (computed from the
 * descriptors the RUNNING binary mounts) in
 * `/.well-known/task-server.json` under `schema_stamps`; this
 * module folds the same stamp from the GENERATED descriptors this
 * bundle was built with and compares. A mismatch means the running
 * task-server predates (or postdates) a `*-proto` change — the
 * state that otherwise surfaces as opaque `Unknown method` /
 * `Invalid payload` RpcErrors.
 */
import { RpcError, RpcErrorCode, type ServiceDescriptor } from "@bearcove/vox-core";

import { authService_descriptor } from "@/generated/authservice.generated";
import { milestoneServiceRpc_descriptor } from "@/generated/milestoneservicerpc.generated";
import { projectServiceRpc_descriptor } from "@/generated/projectservicerpc.generated";
import { taskServiceRpc_descriptor } from "@/generated/taskservicerpc.generated";
import { taskServiceStream_descriptor } from "@/generated/taskservicestream.generated";
import { workstreamServiceRpc_descriptor } from "@/generated/workstreamservicerpc.generated";
import { workstreamServiceStream_descriptor } from "@/generated/workstreamservicestream.generated";

/** Every generated descriptor this bundle ships. */
export const GENERATED_DESCRIPTORS: ServiceDescriptor[] = [
  authService_descriptor,
  milestoneServiceRpc_descriptor,
  projectServiceRpc_descriptor,
  taskServiceRpc_descriptor,
  taskServiceStream_descriptor,
  workstreamServiceRpc_descriptor,
  workstreamServiceStream_descriptor,
];

/**
 * Stamp one service: XOR of its method ids as 16 lowercase hex
 * digits — byte-identical to `org_proto::schema_stamp::service_stamp`.
 */
export function serviceStamp(descriptor: ServiceDescriptor): string {
  let acc = 0n;
  for (const id of descriptor.methods.keys()) acc ^= id;
  return acc.toString(16).padStart(16, "0");
}

export interface SchemaCheck {
  /** Services whose stamp differs from the server's — skew. */
  stale: string[];
  /** Services the server doesn't stamp (older server, or service
   *  added since its build). */
  unverified: string[];
  /** Services whose stamps match. */
  ok: string[];
}

/**
 * Compare this bundle's generated descriptors against the
 * `schema_stamps` map out of the well-known document. Pass `null`
 * /`undefined` when the server didn't send one (predates the
 * guard) — everything lands in `unverified`.
 */
export function checkSchemaStamps(
  served: Record<string, string> | null | undefined,
  descriptors: ServiceDescriptor[] = GENERATED_DESCRIPTORS,
): SchemaCheck {
  const out: SchemaCheck = { stale: [], unverified: [], ok: [] };
  for (const d of descriptors) {
    const want = serviceStamp(d);
    const got = served?.[d.service_name];
    if (got === undefined) out.unverified.push(d.service_name);
    else if (got === want) out.ok.push(d.service_name);
    else out.stale.push(d.service_name);
  }
  return out;
}

/**
 * `true` for the errors schema skew produces:
 *
 * - `RpcError` UNKNOWN_METHOD / INVALID_PAYLOAD — the server
 *   doesn't know the (shape-hashed) method id, or refused the
 *   payload;
 * - vox-postcard's `TranslationError` ("required field X missing
 *   from remote schema Y") — the schema translation layer caught
 *   the struct mismatch before the call even decoded.
 *
 * Used to turn an opaque protocol error into a "rebuild
 * task-server" diagnosis when the stamp check already said the
 * service is stale/unverified. Matched by error name (the
 * vox-postcard class isn't re-exported through vox-core).
 */
export function isLikelySkewError(e: unknown): boolean {
  if (
    e instanceof RpcError &&
    (e.code === RpcErrorCode.UNKNOWN_METHOD ||
      e.code === RpcErrorCode.INVALID_PAYLOAD)
  ) {
    return true;
  }
  return (
    e instanceof Error &&
    (e.name === "TranslationError" ||
      /missing from remote schema|structural mismatch/i.test(e.message))
  );
}
