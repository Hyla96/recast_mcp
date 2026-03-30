import { create } from 'zustand';
import { immer } from 'zustand/middleware/immer';

// ─── URL Slice ─────────────────────────────────────────────────────────────

export type HttpMethod = 'GET' | 'POST' | 'PUT' | 'DELETE' | 'PATCH';

export type ParamType = 'string' | 'number' | 'boolean';

export interface PathParam {
  name: string;
  type: ParamType;
  example: string;
}

export interface QueryParam {
  key: string;
  type: ParamType;
  rawValue: string;
}

export interface UrlSlice {
  url: string;
  method: HttpMethod;
  pathParams: PathParam[];
  queryParams: QueryParam[];
  isValid: boolean;
}

// ─── Auth Slice ─────────────────────────────────────────────────────────────

export type AuthType = 'none' | 'bearer' | 'api-key' | 'basic';
export type ApiKeyPlacement = 'header' | 'query';

export interface AuthSlice {
  type: AuthType;
  bearerToken: string;
  apiKeyName: string;
  apiKeyValue: string;
  apiKeyPlacement: ApiKeyPlacement;
  basicUsername: string;
  basicPassword: string;
}

// ─── Test Slice ─────────────────────────────────────────────────────────────

export type TestOutcome = 'idle' | 'loading' | 'success' | 'error' | 'timeout' | 'network-error';

export interface TestSlice {
  outcome: TestOutcome;
  statusCode: number | null;
  response: unknown;
  isUnverified: boolean;
  sampleJson: string | null;
}

// ─── Mapping Slice ──────────────────────────────────────────────────────────

export interface SelectedField {
  jsonPath: string;
  name: string;
  type: string;
  example: string;
}

export interface MappingSlice {
  selectedFields: SelectedField[];
}

// ─── Naming Slice ───────────────────────────────────────────────────────────

export interface NamingSlice {
  toolName: string;
  toolDescription: string;
}

// ─── Request Body Slice ─────────────────────────────────────────────────────

export interface RequestBodySlice {
  requestBody: string | null;
}

// ─── Stage ──────────────────────────────────────────────────────────────────

export type BuilderStage = 'url' | 'auth' | 'test' | 'mapping' | 'naming' | 'review';

// ─── Combined Store ──────────────────────────────────────────────────────────

export interface BuilderState {
  currentStage: BuilderStage;
  stageValidation: Record<BuilderStage, boolean>;

  urlSlice: UrlSlice;
  authSlice: AuthSlice;
  testSlice: TestSlice;
  mappingSlice: MappingSlice;
  namingSlice: NamingSlice;
  requestBodySlice: RequestBodySlice;

  // Actions
  setCurrentStage: (stage: BuilderStage) => void;
  setStageValid: (stage: BuilderStage, valid: boolean) => void;

  setUrl: (url: string) => void;
  setMethod: (method: HttpMethod) => void;
  setPathParams: (params: PathParam[]) => void;
  setQueryParams: (params: QueryParam[]) => void;
  setUrlValid: (valid: boolean) => void;

  setAuthType: (type: AuthType) => void;
  setBearerToken: (token: string) => void;
  setApiKeyName: (name: string) => void;
  setApiKeyValue: (value: string) => void;
  setApiKeyPlacement: (placement: ApiKeyPlacement) => void;
  setBasicUsername: (username: string) => void;
  setBasicPassword: (password: string) => void;

  setTestOutcome: (outcome: TestOutcome) => void;
  setTestStatusCode: (code: number | null) => void;
  setTestResponse: (response: unknown) => void;
  setIsUnverified: (unverified: boolean) => void;
  setSampleJson: (json: string | null) => void;

  addSelectedField: (field: SelectedField) => void;
  removeSelectedField: (jsonPath: string) => void;
  reorderSelectedFields: (fields: SelectedField[]) => void;
  updateFieldName: (jsonPath: string, name: string) => void;

  setToolName: (name: string) => void;
  setToolDescription: (description: string) => void;

  setRequestBody: (body: string | null) => void;

  resetBuilder: () => void;
}

const initialUrlSlice: UrlSlice = {
  url: '',
  method: 'GET',
  pathParams: [],
  queryParams: [],
  isValid: false,
};

const initialAuthSlice: AuthSlice = {
  type: 'none',
  bearerToken: '',
  apiKeyName: '',
  apiKeyValue: '',
  apiKeyPlacement: 'header',
  basicUsername: '',
  basicPassword: '',
};

const initialTestSlice: TestSlice = {
  outcome: 'idle',
  statusCode: null,
  response: null,
  isUnverified: false,
  sampleJson: null,
};

const initialMappingSlice: MappingSlice = {
  selectedFields: [],
};

const initialNamingSlice: NamingSlice = {
  toolName: '',
  toolDescription: '',
};

const initialRequestBodySlice: RequestBodySlice = {
  requestBody: null,
};

const initialStageValidation: Record<BuilderStage, boolean> = {
  url: false,
  auth: true,
  test: false,
  mapping: false,
  naming: false,
  review: false,
};

export const useBuilderStore = create<BuilderState>()(
  immer((set) => ({
    currentStage: 'url',
    stageValidation: initialStageValidation,

    urlSlice: { ...initialUrlSlice },
    authSlice: { ...initialAuthSlice },
    testSlice: { ...initialTestSlice },
    mappingSlice: { ...initialMappingSlice },
    namingSlice: { ...initialNamingSlice },
    requestBodySlice: { ...initialRequestBodySlice },

    setCurrentStage: (stage) =>
      set((state) => {
        state.currentStage = stage;
      }),

    setStageValid: (stage, valid) =>
      set((state) => {
        state.stageValidation[stage] = valid;
      }),

    setUrl: (url) =>
      set((state) => {
        state.urlSlice.url = url;
      }),

    setMethod: (method) =>
      set((state) => {
        state.urlSlice.method = method;
        if (method === 'GET' || method === 'DELETE') {
          state.requestBodySlice.requestBody = null;
        }
      }),

    setPathParams: (params) =>
      set((state) => {
        state.urlSlice.pathParams = params;
      }),

    setQueryParams: (params) =>
      set((state) => {
        state.urlSlice.queryParams = params;
      }),

    setUrlValid: (valid) =>
      set((state) => {
        state.urlSlice.isValid = valid;
        state.stageValidation.url = valid;
      }),

    setAuthType: (type) =>
      set((state) => {
        state.authSlice.type = type;
      }),

    setBearerToken: (token) =>
      set((state) => {
        state.authSlice.bearerToken = token;
      }),

    setApiKeyName: (name) =>
      set((state) => {
        state.authSlice.apiKeyName = name;
      }),

    setApiKeyValue: (value) =>
      set((state) => {
        state.authSlice.apiKeyValue = value;
      }),

    setApiKeyPlacement: (placement) =>
      set((state) => {
        state.authSlice.apiKeyPlacement = placement;
      }),

    setBasicUsername: (username) =>
      set((state) => {
        state.authSlice.basicUsername = username;
      }),

    setBasicPassword: (password) =>
      set((state) => {
        state.authSlice.basicPassword = password;
      }),

    setTestOutcome: (outcome) =>
      set((state) => {
        state.testSlice.outcome = outcome;
      }),

    setTestStatusCode: (code) =>
      set((state) => {
        state.testSlice.statusCode = code;
      }),

    setTestResponse: (response) =>
      set((state) => {
        state.testSlice.response = response;
      }),

    setIsUnverified: (unverified) =>
      set((state) => {
        state.testSlice.isUnverified = unverified;
      }),

    setSampleJson: (json) =>
      set((state) => {
        state.testSlice.sampleJson = json;
      }),

    addSelectedField: (field) =>
      set((state) => {
        const exists = state.mappingSlice.selectedFields.some(
          (f) => f.jsonPath === field.jsonPath
        );
        if (!exists) {
          state.mappingSlice.selectedFields.push(field);
        }
      }),

    removeSelectedField: (jsonPath) =>
      set((state) => {
        state.mappingSlice.selectedFields = state.mappingSlice.selectedFields.filter(
          (f) => f.jsonPath !== jsonPath
        );
      }),

    reorderSelectedFields: (fields) =>
      set((state) => {
        state.mappingSlice.selectedFields = fields;
      }),

    updateFieldName: (jsonPath, name) =>
      set((state) => {
        const field = state.mappingSlice.selectedFields.find((f) => f.jsonPath === jsonPath);
        if (field !== undefined) {
          field.name = name;
        }
      }),

    setToolName: (name) =>
      set((state) => {
        state.namingSlice.toolName = name;
      }),

    setToolDescription: (description) =>
      set((state) => {
        state.namingSlice.toolDescription = description;
      }),

    setRequestBody: (body) =>
      set((state) => {
        state.requestBodySlice.requestBody = body;
      }),

    resetBuilder: () =>
      set((state) => {
        state.currentStage = 'url';
        state.stageValidation = { ...initialStageValidation };
        state.urlSlice = { ...initialUrlSlice };
        state.authSlice = { ...initialAuthSlice };
        state.testSlice = { ...initialTestSlice };
        state.mappingSlice = { ...initialMappingSlice };
        state.namingSlice = { ...initialNamingSlice };
        state.requestBodySlice = { ...initialRequestBodySlice };
      }),
  }))
);
