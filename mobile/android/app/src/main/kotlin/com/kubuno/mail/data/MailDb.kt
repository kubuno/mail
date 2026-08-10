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
 * A cached thread, so the inbox opens instantly and works offline.
 *
 * [accountKey] scopes rows to one account (serverUrl|userId): the device may
 * hold several, and their thread ids are only unique within an instance.
 * [orderKey] is `last_message_at`, used both to sort and as the keyset cursor.
 */
@Entity(tableName = "threads", primaryKeys = ["accountKey", "id"])
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

@Dao
interface MailDao {
    @Query(
        "SELECT * FROM threads WHERE accountKey = :accountKey AND folder = :folder " +
            "ORDER BY orderKey DESC"
    )
    fun observe(accountKey: String, folder: String): Flow<List<ThreadEntity>>

    @Insert(onConflict = OnConflictStrategy.REPLACE)
    suspend fun upsert(threads: List<ThreadEntity>)

    @Query("DELETE FROM threads WHERE accountKey = :accountKey AND folder = :folder")
    suspend fun clearFolder(accountKey: String, folder: String)

    @Query("DELETE FROM threads WHERE accountKey = :accountKey AND id = :id")
    suspend fun delete(accountKey: String, id: String)

    @Query("UPDATE threads SET isStarred = :starred WHERE accountKey = :accountKey AND id = :id")
    suspend fun setStarred(accountKey: String, id: String, starred: Boolean)
}

@Database(entities = [ThreadEntity::class], version = 1, exportSchema = false)
abstract class MailDatabase : RoomDatabase() {
    abstract fun dao(): MailDao
}
