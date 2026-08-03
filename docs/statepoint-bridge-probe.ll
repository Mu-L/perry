; Minimal Perry-shaped statepoint probe.
;
; A live NaN-boxed value is carried as an addrspace(1) pointer only across
; the safepoint. The runtime may rewrite its spill slot, gc.relocate reloads
; it, and ptrtoint restores Perry's existing i64 representation.

target triple = "arm64-apple-macosx15.0.0"

declare i64 @may_collect(i64)
declare token @llvm.experimental.gc.statepoint.p0(i64 immarg, i32 immarg, ptr, i32 immarg, i32 immarg, ...)
declare i64 @llvm.experimental.gc.result.i64(token)
declare ptr addrspace(1) @llvm.experimental.gc.relocate.p1(token, i32 immarg, i32 immarg)

define i64 @statepoint_bridge_probe(i64 %bits, i64 %arg) gc "statepoint-example" {
entry:
  %root = inttoptr i64 %bits to ptr addrspace(1)
  %statepoint = call token (i64, i32, ptr, i32, i32, ...) @llvm.experimental.gc.statepoint.p0(
      i64 1,
      i32 0,
      ptr elementtype(i64 (i64)) @may_collect,
      i32 1,
      i32 0,
      i64 %arg,
      i32 0,
      i32 0
    ) ["gc-live"(ptr addrspace(1) %root)]
  %result = call i64 @llvm.experimental.gc.result.i64(token %statepoint)
  %root.relocated = call ptr addrspace(1) @llvm.experimental.gc.relocate.p1(
      token %statepoint,
      i32 0,
      i32 0
    )
  %bits.relocated = ptrtoint ptr addrspace(1) %root.relocated to i64
  %combined = xor i64 %result, %bits.relocated
  ret i64 %combined
}
