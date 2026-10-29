// @ts-nocheck — vendored from github.com/openooxml/betteroffice packages/agents (Apache-2.0);
// upstream builds under a looser tsconfig. Keep diffs minimal for upstream merges.

export { DocxAgentDocument, openDocx, attachDocx } from './document.ts';
export * from './types.ts';
export { XlsxAgentWorkbook, openXlsx, type CellEdit, type CellChange } from './xlsx.ts';
export { PptxAgentPresentation, openPptx, type SlideTextChange } from './pptx.ts';
export type { AgentProposal, PrototypeOptions } from './prototype.ts';
