export interface LoginRequest {
  token: string
}

export interface LoginResponse {
  success: boolean
  message: string
}

export interface SessionResponse {
  valid: boolean
  expires_at: string | null
}
