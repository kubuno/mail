package com.kubuno.mail.net

import retrofit2.http.Body
import retrofit2.http.DELETE
import retrofit2.http.GET
import retrofit2.http.POST
import retrofit2.http.Path
import retrofit2.http.Query

/**
 * The mail module's HTTP surface, under /api/v1/mail (the core proxies the
 * prefix to the module). Written by hand: api-spec has no generated mail client.
 * Only the read paths M1 needs are here; actions and send land with M3.
 */
interface MailApi {

    @GET("api/v1/mail/accounts")
    suspend fun accounts(): MailAccountsDto

    @GET("api/v1/mail/threads")
    suspend fun threads(
        @Query("folder") folder: String = "inbox",
        @Query("account_id") accountId: String? = null,
        @Query("category") category: String? = null,
        @Query("limit") limit: Int = 50,
        // Keyset cursor: the last thread's last_message_at from the prior page.
        @Query("before") before: String? = null,
    ): ThreadPageDto

    /** Delta since the client's modseq cursor. Requires the deployed module. */
    @GET("api/v1/mail/changes")
    suspend fun changes(
        @Query("since") since: Long,
        @Query("limit") limit: Int = 200,
        @Query("account_id") accountId: String? = null,
    ): ChangesDto

    @GET("api/v1/mail/threads/{id}")
    suspend fun thread(@Path("id") id: String): ThreadDetailDto

    /** Move a thread to another folder (inbox|sent|spam|trash|archive). */
    @POST("api/v1/mail/threads/{id}/move")
    suspend fun move(@Path("id") id: String, @Body body: MoveBody)

    /** Soft-delete: the server moves the thread to trash. */
    @DELETE("api/v1/mail/threads/{id}")
    suspend fun trash(@Path("id") id: String)

    /** Toggle the star (the server flips it; no explicit set). */
    @POST("api/v1/mail/threads/{id}/star")
    suspend fun toggleStar(@Path("id") id: String): StarResult
}
