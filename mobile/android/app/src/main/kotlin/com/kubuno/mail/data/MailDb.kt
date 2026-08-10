package com.kubuno.mail.data

import androidx.room.Dao
import androidx.room.Database
import androidx.room.Entity
import androidx.room.Insert
import androidx.room.OnConflictStrategy
import androidx.room.Query
import androidx.room.RoomDatabase
import kotlinx.coroutines.flow.Flow

/**
 * A cached thread row, so the inbox opens instantly and works offline.
 *
 * The primary key includes [folder]: one thread can appear in several views at
 * once (a starred inbox thread is in both Réception and Suivis), so it needs a
 * row per view. [accountKey] scopes rows to one account (serverUrl|userId).
 * [orderKey] is `last_message_at`, used to sort.
 */
@Entity(tableName = "threads", primaryKeys = ["accountKey", "folder", "id"])
data class ThreadEntity(
    val accountKey: String,
    val id: String,
    val folder: String,
    val subject: String?,
    val snippet: String?,
    val senderName: String?,
    val senderEmail: String?,
    val orderKey: String?,
    val unreadCount: Int,
    val isStarred: Boolean,
    val isImportant: Boolean,
    val hasAttachments: Boolean,
    val category: String?,
)

/** The delta cursor (max modseq applied) for one account. */
@Entity(tableName = "sync_state", primaryKeys = ["accountKey"])
data class SyncStateEntity(
    val accountKey: String,
    val cursor: String,
)

@Dao
interface MailDao {
    @Query(
        "SELECT * FROM threads WHERE accountKey = :accountKey AND folder = :folder " +
            "ORDER BY orderKey DESC"
    )
    fun observe(accountKey: String, folder: String): Flow<List<ThreadEntity>>

    @Query("SELECT * FROM threads WHERE accountKey = :accountKey AND id = :id")
    suspend fun rowsFor(accountKey: String, id: String): List<ThreadEntity>

    @Insert(onConflict = OnConflictStrategy.REPLACE)
    suspend fun upsert(threads: List<ThreadEntity>)

    @Query("DELETE FROM threads WHERE accountKey = :accountKey AND folder = :folder")
    suspend fun clearFolder(accountKey: String, folder: String)

    /** Remove the thread from one view only (it left that folder). */
    @Query("DELETE FROM threads WHERE accountKey = :accountKey AND folder = :folder AND id = :id")
    suspend fun deleteInFolder(accountKey: String, folder: String, id: String)

    /** Remove the thread from every view (it was deleted). */
    @Query("DELETE FROM threads WHERE accountKey = :accountKey AND id = :id")
    suspend fun delete(accountKey: String, id: String)

    @Query("UPDATE threads SET unreadCount = :count WHERE accountKey = :accountKey AND id = :id")
    suspend fun setUnread(accountKey: String, id: String, count: Int)

    @Query("SELECT cursor FROM sync_state WHERE accountKey = :accountKey")
    suspend fun cursor(accountKey: String): String?

    @Insert(onConflict = OnConflictStrategy.REPLACE)
    suspend fun setCursor(state: SyncStateEntity)
}

@Database(entities = [ThreadEntity::class, SyncStateEntity::class], version = 2, exportSchema = false)
abstract class MailDatabase : RoomDatabase() {
    abstract fun dao(): MailDao
}
