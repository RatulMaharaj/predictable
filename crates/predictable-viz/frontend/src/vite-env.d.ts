/// <reference types="vite/client" />
declare module "elkjs/lib/elk-api.js" {
  const ELK: new (options?: { workerFactory?: () => Worker }) => {
    layout(graph: unknown, options?: unknown): Promise<unknown>;
  };
  export default ELK;
}
