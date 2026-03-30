/**
 * Types for MCP server resources returned by the Platform API.
 */

export type ServerStatus = 'active' | 'error' | 'inactive';

export interface McpServer {
  id: string;
  name: string;
  slug: string;
  status: ServerStatus;
  /** Full MCP endpoint URL for this server */
  endpointUrl: string;
  /** ISO 8601 timestamp of the most recent tool call, or null if never called */
  lastCallAt: string | null;
  /** Number of tool calls in the past 24 hours */
  callCount24h: number;
  updatedAt: string;
  createdAt: string;
}

export interface ServersListResponse {
  data: McpServer[];
  pagination: {
    total: number;
    page: number;
    pageSize: number;
  };
}
