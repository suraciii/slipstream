export type EditorProxyViewModel = Readonly<{
  state: "absent" | "building" | "current" | "stale";
  note: string;
  proxy: Readonly<{
    width: number;
    height: number;
    longEdge: number;
    qualityLimit: string;
    byteLength: number;
    sourceRevision: string;
    sourceProfileId: string;
    pipelineVersion: string;
  }> | null;
  canCreate: boolean;
  canRemove: boolean;
}>;
