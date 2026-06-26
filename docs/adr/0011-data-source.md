# ADR-0010: Cargo Workspace Structure

## Status
Accepted

## Context
There are multiple potential sources for NEXRAD Level II data. The two leading options are NOAA via AWS S3 (`s3://noaa-nexrad-level2`), or Iowa State Mesonet. 

A decision was required on which data source to use for initial development, and as the default provider. 

## Decision
The project uses NOAA via AWS S3 as the primary source of data. However, this is currently accomplished by the user passing in the appropriate URL into a fetch_sample binary as an argument. This is a development scaffold and will not be the final solution. However, data_acquisition.rs is expected to be reused in later iterations. It currently accepts a URL and downloads the file located there. This will be repurposed to work programmatically, but will likely remain simple/direct, in that it will likely not use AWS SDK. 

## Consequences
- Tests and development will proceed under the assumption that the NOAA S3 bucket will be the default data source. 
- Later expansions to backup sources will need to be considered and implemented. 
- Current manual implementation leaves the door open for later programmatic implementation. 

