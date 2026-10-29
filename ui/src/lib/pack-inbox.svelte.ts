// A `.alpack` the OS asked the app to open (file association): the Library takes it and opens the review screen.
class PackInbox {
  path = $state<string | null>(null);
}
export const packInbox = new PackInbox();
