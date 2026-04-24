# UI design

Use a grid, With several fixed column and a dynamic column.

## fixed column

1. Up time: In second. Since the request was received.
2. Upload size: In Kilobyte. 
3. Download size
4. Status: With emoji showing the `waiting` `connected` `completed (error)`, and `completed (normally)`
5. Local IP to remote URI. (make it align http, https, socks5(tcp/udp))

##  dynamic column 

+ Upon retry, append additional information at the end. 
  + For example, when the first time it fails, it should look like `🔁 [error details]`
  + At the third time it fails it look like `🔁🔁🔁 [last time error details]`
+ When the status changes to error it also shows the details of error.

## Order

When a request was received, Add it to the head of the grid. When the request was complete, both error or normal. keep it for three seconds, then delete it.

# IMPORTANT

This may need a fundamental change in the representation of a task. It is allowed!.
