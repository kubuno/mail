package com.kubuno.mail.di

import android.content.Context
import androidx.room.Room
import com.kubuno.mail.data.MailDatabase
import dagger.Module
import dagger.Provides
import dagger.hilt.InstallIn
import dagger.hilt.android.qualifiers.ApplicationContext
import dagger.hilt.components.SingletonComponent
import javax.inject.Singleton

@Module
@InstallIn(SingletonComponent::class)
object MailModule {

    @Provides
    @Singleton
    fun provideDatabase(@ApplicationContext context: Context): MailDatabase =
        Room.databaseBuilder(context, MailDatabase::class.java, "kubuno-mail.db")
            // Pre-release cache: recreating it is cheap and it is only a mirror
            // of the server, refetched on the next refresh.
            .fallbackToDestructiveMigration(dropAllTables = true)
            .build()
}
